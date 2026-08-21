pub mod inbound;
pub mod runtime;
pub mod tasks;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::clients::{ApolloClient, TavilyClient};
use crate::config::{AppConfig, IcpCriteria};
use crate::connectors::gmail::{GmailConnector, OauthClient, load_senders};
use crate::connectors::health::{DbProbe, HealthInputs, HealthReport, run_health_check};
use crate::connectors::{LogNotifier, Notifier, NotifyLevel};
use crate::db::companies;
use crate::llm::{LlmClient, Policy};
use crate::memory::MemoryClient;
use crate::workflows::{discovery, outreach, reporting, research, sync};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    CsvSync,
    DiscoverContacts,
    EnrichContacts,
    EnrichCompanies,
    ResearchCompanies,
    DetectSignals,
    ReplyMonitor,
    OutreachSequence,
    OutreachSend,
    TaskExecutor,
    DailyDigest,
    WeeklyReport,
    HealthCheck,
}

impl JobKind {
    pub const ALL: [JobKind; 13] = [
        JobKind::CsvSync,
        JobKind::DiscoverContacts,
        JobKind::EnrichContacts,
        JobKind::EnrichCompanies,
        JobKind::ResearchCompanies,
        JobKind::DetectSignals,
        JobKind::ReplyMonitor,
        JobKind::OutreachSequence,
        JobKind::OutreachSend,
        JobKind::TaskExecutor,
        JobKind::DailyDigest,
        JobKind::WeeklyReport,
        JobKind::HealthCheck,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::CsvSync => "csv_sync",
            JobKind::DiscoverContacts => "discover_contacts",
            JobKind::EnrichContacts => "enrich_contacts",
            JobKind::EnrichCompanies => "enrich_companies",
            JobKind::ResearchCompanies => "research_companies",
            JobKind::DetectSignals => "detect_signals",
            JobKind::ReplyMonitor => "reply_monitor",
            JobKind::OutreachSequence => "outreach_sequence",
            JobKind::OutreachSend => "outreach_send",
            JobKind::TaskExecutor => "task_executor",
            JobKind::DailyDigest => "daily_digest",
            JobKind::WeeklyReport => "weekly_report",
            JobKind::HealthCheck => "health_check",
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for JobKind {
    type Err = JobError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == raw)
            .ok_or_else(|| JobError::Unknown(raw.to_string()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("unknown job {0}")]
    Unknown(String),
    #[error("sync error: {0}")]
    Sync(#[from] sync::SyncError),
    #[error("discovery error: {0}")]
    Discovery(#[from] discovery::DiscoveryError),
    #[error("research error: {0}")]
    Research(#[from] research::ResearchError),
    #[error("report error: {0}")]
    Report(#[from] reporting::ReportError),
    #[error("report encoding failed: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("queue error: {0}")]
    Queue(#[from] sqlx::Error),
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("outreach error: {0}")]
    Outreach(#[from] crate::workflows::outreach::OutreachError),
    #[error("every discovery search failed across {attempted} companies")]
    DiscoveryUnavailable { attempted: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedJob {
    pub name: JobKind,
}

pub struct JobContext {
    pub pool: PgPool,
    pub config: AppConfig,
    pub memory: MemoryClient,
    pub llm: LlmClient,
    pub apollo: ApolloClient,
    pub tavily: TavilyClient,
    pub policies: Vec<Policy>,
    pub notifier: LogNotifier,
    pub gmail: Option<GmailConnector>,
    pub oauth: Option<OauthClient>,
}

impl JobContext {
    pub fn from_config(pool: PgPool, config: AppConfig) -> Self {
        let memory = MemoryClient::new(&config.memory);
        let llm = LlmClient::new(&config.llm);
        let apollo = ApolloClient::new(config.apollo_api_key.clone());
        let tavily = TavilyClient::new(config.tavily_api_key.clone());
        let oauth = config
            .email
            .gmail
            .as_ref()
            .and_then(|gmail_config| OauthClient::from_file(&gmail_config.client_file).ok());
        let gmail = config.email.gmail.as_ref().and_then(|gmail_config| {
            let client = oauth.clone()?;
            let senders = load_senders(&gmail_config.senders_file).ok()?;
            if senders.is_empty() {
                return None;
            }
            Some(GmailConnector::new(client, senders, pool.clone()))
        });
        Self {
            pool,
            config,
            memory,
            llm,
            apollo,
            tavily,
            policies: crate::llm::default_policies(),
            notifier: LogNotifier,
            gmail,
            oauth,
        }
    }
}

const RESEARCH_BATCH: i64 = 3;
const DISCOVER_BATCH: i64 = 5;
const ENROLL_LIMIT: i64 = 50;
const SEND_LIMIT: i64 = 50;
const ENRICH_LIMIT: i64 = 25;
const ENRICH_CREDITS: u16 = 10;

pub async fn health_report(ctx: &JobContext) -> HealthReport {
    let senders = ctx
        .gmail
        .as_ref()
        .map(|connector| connector.senders().to_vec())
        .unwrap_or_default();
    run_health_check(&HealthInputs {
        db: DbProbe::Pool(&ctx.pool),
        llm_base_url: Some(&ctx.config.llm.base_url),
        memory_base_url: Some(&ctx.config.memory.base_url),
        gmail: ctx.oauth.as_ref(),
        senders: &senders,
    })
    .await
}

pub async fn run_job(ctx: &JobContext, kind: JobKind) -> Result<serde_json::Value, JobError> {
    match kind {
        JobKind::CsvSync => {
            let report = sync::sync_dir(&ctx.pool, &ctx.memory, &ctx.config.csv_data_dir).await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::DiscoverContacts => discover_batch(ctx).await,
        JobKind::EnrichContacts => {
            let mut credits = ENRICH_CREDITS;
            let report =
                discovery::enrich_contacts(&ctx.pool, &ctx.memory, &ctx.apollo, ENRICH_LIMIT, &mut credits)
                    .await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::EnrichCompanies => {
            let mut credits = ENRICH_CREDITS;
            let report =
                discovery::enrich_companies(&ctx.pool, &ctx.memory, &ctx.apollo, ENRICH_LIMIT, &mut credits)
                    .await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::OutreachSequence => {
            let report =
                outreach::enroll_contacts(&ctx.pool, &crate::config::Cadence::standard(), ENROLL_LIMIT)
                    .await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::OutreachSend => {
            let inputs = outreach::SendPassInputs {
                memory: &ctx.memory,
                llm: &ctx.llm,
                policies: &ctx.policies,
                rules: &crate::config::MessagingRules::default(),
                preflight_config: &crate::workflows::accounts::PreflightConfig::default(),
                transport: ctx.gmail.as_ref(),
                dry_run: ctx.config.dry_run,
                limit: SEND_LIMIT,
            };
            let report = outreach::run_send_pass(&ctx.pool, &inputs, chrono::Utc::now()).await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::ReplyMonitor => {
            let report = inbound::poll_replies(ctx).await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::TaskExecutor => {
            let report = tasks::execute_due(ctx).await?;
            Ok(serde_json::to_value(report)?)
        }
        JobKind::ResearchCompanies => research_batch(ctx).await,
        JobKind::DetectSignals => signal_batch(ctx).await,
        JobKind::DailyDigest => activity_report(ctx, 1, "daily digest").await,
        JobKind::WeeklyReport => activity_report(ctx, 7, "weekly report").await,
        JobKind::HealthCheck => {
            let report = health_report(ctx).await;
            Ok(serde_json::to_value(report)?)
        }
    }
}

async fn activity_report(
    ctx: &JobContext,
    window_days: i32,
    label: &str,
) -> Result<serde_json::Value, JobError> {
    let report = reporting::activity_report(&ctx.pool, window_days).await?;
    if let Err(e) = ctx
        .notifier
        .notify(NotifyLevel::Info, &report.render_with_label(label))
        .await
    {
        tracing::warn!(error = %e, label, "activity report notify failed");
    }
    Ok(serde_json::to_value(report)?)
}

async fn discover_batch(ctx: &JobContext) -> Result<serde_json::Value, JobError> {
    let candidates = companies::list_without_contacts(&ctx.pool, DISCOVER_BATCH).await?;
    let icp = IcpCriteria::default();
    let budget = discovery::DiscoveryBudget::default();
    let mut credits = budget.max_credits_per_run;
    let mut attempted: Vec<String> = Vec::new();
    let mut searches_failed = 0usize;
    let mut total = discovery::DiscoveryReport::default();
    for company in candidates {
        if credits == 0 {
            break;
        }
        let report = discovery::discover_contacts(
            &ctx.pool,
            &ctx.memory,
            &ctx.apollo,
            &icp,
            &company.domain,
            &budget,
            &mut credits,
        )
        .await?;
        if report.search_failed == 0 {
            companies::mark_discovery_attempted(&ctx.pool, &company.domain).await?;
        }
        if report.search_failed > 0 {
            searches_failed += 1;
        }
        total.absorb(&report);
        attempted.push(company.domain);
    }
    if !attempted.is_empty() && searches_failed == attempted.len() {
        return Err(JobError::DiscoveryUnavailable {
            attempted: attempted.len(),
        });
    }
    Ok(serde_json::json!({
        "attempted": attempted,
        "report": serde_json::to_value(total)?,
    }))
}

async fn research_batch(ctx: &JobContext) -> Result<serde_json::Value, JobError> {
    let companies = crate::db::companies::list_unenriched(&ctx.pool, RESEARCH_BATCH)
        .await
        .map_err(research::ResearchError::from)
        .map_err(JobError::from)?;
    let mut researched = Vec::new();
    for company in companies {
        match research::research_company(
            &ctx.pool,
            &ctx.memory,
            &ctx.tavily,
            &ctx.llm,
            &ctx.policies,
            &company.domain,
        )
        .await
        {
            Ok(outcome) => researched.push(outcome.domain),
            Err(e) => {
                tracing::warn!(domain = company.domain, error = %e, "research job item failed");
            }
        }
    }
    Ok(serde_json::json!({ "researched": researched }))
}

async fn signal_batch(ctx: &JobContext) -> Result<serde_json::Value, JobError> {
    let companies = crate::db::companies::list_recent(&ctx.pool, RESEARCH_BATCH)
        .await
        .map_err(research::ResearchError::from)
        .map_err(JobError::from)?;
    let mut ingested = 0u64;
    for company in companies {
        match research::detect_signals(
            &ctx.pool,
            &ctx.memory,
            &ctx.tavily,
            &ctx.llm,
            &ctx.policies,
            &company.domain,
        )
        .await
        {
            Ok(report) => ingested += report.ingested,
            Err(e) => {
                tracing::warn!(domain = company.domain, error = %e, "signal job item failed");
            }
        }
    }
    Ok(serde_json::json!({ "signals_ingested": ingested }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn every_kind_round_trips_through_its_name() {
        for kind in JobKind::ALL {
            assert_eq!(JobKind::from_str(kind.as_str()).unwrap(), kind);
            let encoded = serde_json::to_value(kind).unwrap();
            assert_eq!(encoded, serde_json::json!(kind.as_str()));
        }
        assert!(matches!(JobKind::from_str("nope"), Err(JobError::Unknown(_))));
    }
}
