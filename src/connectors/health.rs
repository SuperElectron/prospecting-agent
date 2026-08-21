use std::time::Instant;

use chrono::Utc;
use serde::Serialize;
use sqlx::PgPool;

use crate::connectors::gmail::SenderAccount;
use crate::db;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Degraded,
    Error,
    NotConfigured,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    pub latency_ms: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub status: CheckStatus,
    pub timestamp: chrono::DateTime<Utc>,
    pub checks: Vec<Check>,
}

pub enum DbProbe<'a> {
    Pool(&'a PgPool),
    Unavailable(String),
    NotConfigured,
}

pub struct HealthInputs<'a> {
    pub db: DbProbe<'a>,
    pub llm_base_url: Option<&'a str>,
    pub memory_base_url: Option<&'a str>,
    pub gmail: Option<&'a crate::connectors::gmail::OauthClient>,
    pub senders: &'a [SenderAccount],
}

pub async fn run_health_check(inputs: &HealthInputs<'_>) -> HealthReport {
    let mut checks = Vec::new();
    checks.push(check_db(&inputs.db).await);
    checks.push(check_http("llm", inputs.llm_base_url, "/models").await);
    checks.push(check_http("memory", inputs.memory_base_url, "/api/v1/config/").await);
    checks.push(check_gmail(inputs.gmail, inputs.senders).await);
    let pool = match &inputs.db {
        DbProbe::Pool(pool) => Some(*pool),
        _ => None,
    };
    checks.push(check_capacity(pool, inputs.senders).await);
    let status = if checks.iter().any(|c| c.status == CheckStatus::Error) {
        CheckStatus::Error
    } else if checks
        .iter()
        .any(|c| matches!(c.status, CheckStatus::Degraded | CheckStatus::NotConfigured))
    {
        CheckStatus::Degraded
    } else {
        CheckStatus::Ok
    };
    HealthReport {
        status,
        timestamp: Utc::now(),
        checks,
    }
}

async fn check_db(probe: &DbProbe<'_>) -> Check {
    let start = Instant::now();
    match probe {
        DbProbe::NotConfigured => check(
            "database",
            CheckStatus::NotConfigured,
            start,
            "no url configured".into(),
        ),
        DbProbe::Unavailable(reason) => check("database", CheckStatus::Error, start, reason.clone()),
        DbProbe::Pool(pool) => match sqlx::query("SELECT 1").execute(*pool).await {
            Ok(_) => check("database", CheckStatus::Ok, start, "reachable".into()),
            Err(e) => check("database", CheckStatus::Error, start, e.to_string()),
        },
    }
}

async fn check_gmail(
    oauth: Option<&crate::connectors::gmail::OauthClient>,
    senders: &[SenderAccount],
) -> Check {
    let start = Instant::now();
    let (Some(oauth), Some(sender)) = (oauth, senders.first()) else {
        return check(
            "gmail",
            CheckStatus::NotConfigured,
            start,
            "no oauth client or authorized sender".into(),
        );
    };
    let http = reqwest::Client::new();
    match oauth.access_token(&http, &sender.refresh_token).await {
        Ok(_) => check(
            "gmail",
            CheckStatus::Ok,
            start,
            format!("token refresh ok for {}", sender.email),
        ),
        Err(e) => check("gmail", CheckStatus::Error, start, e.to_string()),
    }
}

async fn check_http(name: &'static str, base_url: Option<&str>, probe_path: &str) -> Check {
    let start = Instant::now();
    let Some(base) = base_url else {
        return check(
            name,
            CheckStatus::NotConfigured,
            start,
            "no url configured".into(),
        );
    };
    let url = format!("{}{probe_path}", base.trim_end_matches('/'));
    let client = reqwest::Client::new();
    match client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            check(name, CheckStatus::Ok, start, format!("{url} reachable"))
        }
        Ok(resp) => check(
            name,
            CheckStatus::Error,
            start,
            format!("{url} returned {}", resp.status().as_u16()),
        ),
        Err(e) => check(name, CheckStatus::Error, start, e.to_string()),
    }
}

async fn check_capacity(pool: Option<&PgPool>, senders: &[SenderAccount]) -> Check {
    let start = Instant::now();
    if senders.is_empty() {
        return check(
            "email_capacity",
            CheckStatus::NotConfigured,
            start,
            "no gmail senders authorized".into(),
        );
    }
    let Some(pool) = pool else {
        return check("email_capacity", CheckStatus::Degraded, start, "no pool".into());
    };
    let day = Utc::now().date_naive();
    let mut total_limit: i64 = 0;
    let mut total_sent: i64 = 0;
    for sender in senders {
        total_limit += i64::from(sender.daily_limit);
        match db::capacity::sent_today(pool, &sender.email, day).await {
            Ok(sent) => total_sent += i64::from(sent),
            Err(e) => return check("email_capacity", CheckStatus::Error, start, e.to_string()),
        }
    }
    let remaining = (total_limit - total_sent).max(0);
    let status = if total_limit > 0 && remaining * 5 < total_limit {
        CheckStatus::Degraded
    } else {
        CheckStatus::Ok
    };
    check(
        "email_capacity",
        status,
        start,
        format!("{remaining}/{total_limit} sends remaining today"),
    )
}

fn check(name: &'static str, status: CheckStatus, start: Instant, detail: String) -> Check {
    Check {
        name,
        status,
        latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn unconfigured_everything_is_degraded_not_error() {
        let inputs = HealthInputs {
            db: DbProbe::NotConfigured,
            llm_base_url: None,
            memory_base_url: None,
            gmail: None,
            senders: &[],
        };
        let report = run_health_check(&inputs).await;
        assert_eq!(report.status, CheckStatus::Degraded);
        assert!(
            report
                .checks
                .iter()
                .all(|c| c.status == CheckStatus::NotConfigured)
        );
    }

    #[tokio::test]
    async fn reachable_llm_reports_ok_and_dead_llm_reports_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let ok = check_http("llm", Some(&server.uri()), "/models").await;
        assert_eq!(ok.status, CheckStatus::Ok);
        let bad = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&bad)
            .await;
        let err = check_http("llm", Some(&bad.uri()), "/models").await;
        assert_eq!(err.status, CheckStatus::Error);
    }

    #[tokio::test]
    async fn error_in_any_check_makes_the_report_unhealthy() {
        let bad = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&bad)
            .await;
        let base = bad.uri();
        let inputs = HealthInputs {
            db: DbProbe::NotConfigured,
            llm_base_url: Some(&base),
            memory_base_url: None,
            gmail: None,
            senders: &[],
        };
        let report = run_health_check(&inputs).await;
        assert_eq!(report.status, CheckStatus::Error);
    }

    #[tokio::test]
    async fn unreachable_database_is_an_error_not_degraded() {
        let inputs = HealthInputs {
            db: DbProbe::Unavailable("connection refused".into()),
            llm_base_url: None,
            memory_base_url: None,
            gmail: None,
            senders: &[],
        };
        let report = run_health_check(&inputs).await;
        assert_eq!(report.status, CheckStatus::Error);
        let db_check = report.checks.iter().find(|c| c.name == "database").unwrap();
        assert_eq!(db_check.status, CheckStatus::Error);
    }
}
