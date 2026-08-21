pub mod runtime;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::clients::{ApolloClient, TavilyClient};
use crate::config::AppConfig;
use crate::connectors::gmail::{GmailConnector, OauthClient, load_senders};
use crate::connectors::{LogNotifier, Notifier, NotifyLevel};
use crate::llm::{LlmClient, Policy};
use crate::memory::MemoryClient;

pub const JOB_NAMES: [&str; 7] = [
    "csv_sync",
    "enrich_contacts",
    "enrich_companies",
    "research_companies",
    "detect_signals",
    "weekly_report",
    "health_check",
];

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("unknown job {0}")]
    Unknown(String),
    #[error("job {name} failed: {reason}")]
    Failed { name: String, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedJob {
    pub name: String,
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
}

impl JobContext {
    pub fn from_config(pool: PgPool, config: AppConfig) -> Self {
        let memory = MemoryClient::new(&config.memory);
        let llm = LlmClient::new(&config.llm);
        let apollo = ApolloClient::new(config.apollo_api_key.clone());
        let tavily = TavilyClient::new(config.tavily_api_key.clone());
        let gmail = config.email.gmail.as_ref().and_then(|gmail_config| {
            let oauth = OauthClient::from_file(&gmail_config.client_file).ok()?;
            let senders = load_senders(&gmail_config.senders_file).ok()?;
            if senders.is_empty() {
                return None;
            }
            Some(GmailConnector::new(oauth, senders, pool.clone()))
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
        }
    }
}

const RESEARCH_BATCH: i64 = 3;
const ENRICH_LIMIT: i64 = 25;
const ENRICH_CREDITS: u16 = 10;

pub async fn run_job(ctx: &JobContext, name: &str) -> Result<serde_json::Value, JobError> {
    let failed = |reason: String| JobError::Failed {
        name: name.to_string(),
        reason,
    };
    match name {
        "csv_sync" => {
            let report = crate::workflows::sync::sync_dir(&ctx.pool, &ctx.memory, &ctx.config.csv_data_dir)
                .await
                .map_err(|e| failed(e.to_string()))?;
            serde_json::to_value(report).map_err(|e| failed(e.to_string()))
        }
        "enrich_contacts" => {
            let mut credits = ENRICH_CREDITS;
            let report = crate::workflows::discovery::enrich_contacts(
                &ctx.pool,
                &ctx.memory,
                &ctx.apollo,
                ENRICH_LIMIT,
                &mut credits,
            )
            .await
            .map_err(|e| failed(e.to_string()))?;
            serde_json::to_value(report).map_err(|e| failed(e.to_string()))
        }
        "enrich_companies" => {
            let mut credits = ENRICH_CREDITS;
            let report = crate::workflows::discovery::enrich_companies(
                &ctx.pool,
                &ctx.memory,
                &ctx.apollo,
                ENRICH_LIMIT,
                &mut credits,
            )
            .await
            .map_err(|e| failed(e.to_string()))?;
            serde_json::to_value(report).map_err(|e| failed(e.to_string()))
        }
        "research_companies" => research_batch(ctx).await.map_err(failed),
        "detect_signals" => signal_batch(ctx).await.map_err(failed),
        "weekly_report" => {
            let report = crate::workflows::reporting::weekly_report(&ctx.pool, 7)
                .await
                .map_err(|e| failed(e.to_string()))?;
            if let Err(e) = ctx.notifier.notify(NotifyLevel::Info, &report.render()).await {
                tracing::warn!(error = %e, "weekly report notify failed");
            }
            serde_json::to_value(report).map_err(|e| failed(e.to_string()))
        }
        "health_check" => {
            let senders = ctx
                .gmail
                .as_ref()
                .map(|g| g.senders().to_vec())
                .unwrap_or_default();
            let oauth = ctx
                .config
                .email
                .gmail
                .as_ref()
                .and_then(|g| OauthClient::from_file(&g.client_file).ok());
            let report =
                crate::connectors::health::run_health_check(&crate::connectors::health::HealthInputs {
                    db: crate::connectors::health::DbProbe::Pool(&ctx.pool),
                    llm_base_url: Some(&ctx.config.llm.base_url),
                    memory_base_url: Some(&ctx.config.memory.base_url),
                    gmail: oauth.as_ref(),
                    senders: &senders,
                })
                .await;
            serde_json::to_value(report).map_err(|e| failed(e.to_string()))
        }
        other => Err(JobError::Unknown(other.to_string())),
    }
}

async fn research_batch(ctx: &JobContext) -> Result<serde_json::Value, String> {
    let companies = crate::db::companies::list_unenriched(&ctx.pool, RESEARCH_BATCH)
        .await
        .map_err(|e| e.to_string())?;
    let mut researched = Vec::new();
    for company in companies {
        match crate::workflows::research::research_company(
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

async fn signal_batch(ctx: &JobContext) -> Result<serde_json::Value, String> {
    let companies = crate::db::companies::list_recent(&ctx.pool, RESEARCH_BATCH)
        .await
        .map_err(|e| e.to_string())?;
    let mut ingested = 0u64;
    for company in companies {
        match crate::workflows::research::detect_signals(
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

    #[test]
    fn job_names_are_unique_and_nonempty() {
        let mut seen = std::collections::HashSet::new();
        for name in JOB_NAMES {
            assert!(!name.is_empty());
            assert!(seen.insert(name));
        }
    }
}
