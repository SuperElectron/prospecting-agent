use std::sync::Arc;

use prospecting_agent::config::AppConfig;
use prospecting_agent::db;
use prospecting_agent::http::webhooks::WebhookRegistry;
use prospecting_agent::http::{AppState, router};
use prospecting_agent::jobs::{JOB_NAMES, JobContext};

fn test_config(database_url: &str) -> AppConfig {
    let env: prospecting_agent::config::EnvMap = [
        ("DATABASE_URL", database_url),
        ("LLM_BASE_URL", "http://127.0.0.1:1/v1"),
        ("LLM_MODEL", "test-model"),
        ("MEMORY_URL", "http://127.0.0.1:1"),
        ("APOLLO_API_KEY", "test-apollo"),
        ("TAVILY_API_KEY", "test-tavily"),
        ("GMAIL_CLIENT_FILE", "/nonexistent/client.json"),
        ("GMAIL_SENDERS_FILE", "/nonexistent/senders.json"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    AppConfig::from_map(&env).expect("test config parses")
}

async fn serve_state(webhooks: WebhookRegistry) -> Option<String> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so http tests cannot pass vacuously"
        );
        return None;
    };
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");
    let ctx = JobContext::from_config(pool, test_config(&url));
    let state = Arc::new(AppState { ctx, webhooks });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.expect("serve");
    });
    Some(format!("http://{addr}"))
}

macro_rules! require_server {
    ($webhooks:expr) => {
        match serve_state($webhooks).await {
            Some(base) => base,
            None => {
                eprintln!("TEST_DATABASE_URL not set; skipping http integration test");
                return;
            }
        }
    };
}

#[tokio::test]
async fn jobs_endpoint_lists_all_jobs_and_webhooks() {
    let mut webhooks = WebhookRegistry::default();
    webhooks.register("test_hook", |_state, _payload| async {
        Ok(serde_json::json!({"ok": true}))
    });
    let base = require_server!(webhooks);
    let body: serde_json::Value = reqwest::get(format!("{base}/control/jobs"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jobs: Vec<&str> = body["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(jobs, JOB_NAMES.to_vec());
    assert_eq!(body["webhooks"], serde_json::json!(["test_hook"]));
}

#[tokio::test]
async fn unknown_job_run_and_enqueue_return_not_found() {
    let base = require_server!(WebhookRegistry::default());
    let client = reqwest::Client::new();
    for path in [
        "/control/jobs/no_such_job/run",
        "/control/jobs/no_such_job/enqueue",
    ] {
        let response = client.post(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn report_endpoint_returns_weekly_counts() {
    let base = require_server!(WebhookRegistry::default());
    let body: serde_json::Value = reqwest::get(format!("{base}/control/report"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for key in [
        "companies_added",
        "contacts_added",
        "emails_sent",
        "replies",
        "opt_outs",
        "sequences_stopped",
        "signals_detected",
    ] {
        assert!(
            body[key].is_u64() || body[key].is_i64(),
            "missing count {key} in {body}"
        );
    }
}

#[tokio::test]
async fn contact_lookup_unknown_email_returns_not_found() {
    let base = require_server!(WebhookRegistry::default());
    let response = reqwest::get(format!("{base}/control/contacts/nobody@nowhere.invalid"))
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webhook_dispatch_routes_by_name() {
    let mut webhooks = WebhookRegistry::default();
    webhooks.register("echo", |_state, payload| async move {
        Ok(serde_json::json!({"echoed": payload}))
    });
    webhooks.register("always_fails", |_state, _payload| async {
        Err("boom".to_string())
    });
    let base = require_server!(webhooks);
    let client = reqwest::Client::new();

    let ok = client
        .post(format!("{base}/webhooks/echo"))
        .json(&serde_json::json!({"k": "v"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = ok.json().await.unwrap();
    assert_eq!(body["echoed"]["k"], "v");

    let failed = client
        .post(format!("{base}/webhooks/always_fails"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = failed.json().await.unwrap();
    assert_eq!(body["error"], "boom");

    let missing = client
        .post(format!("{base}/webhooks/nope"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn queue_enqueue_persists_job_row() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL not set; skipping queue integration test");
        return;
    };
    prospecting_agent::jobs::runtime::enqueue(&url, "health_check")
        .await
        .expect("enqueue succeeds");
    let pool = prospecting_agent::jobs::runtime::queue_pool(&url)
        .await
        .expect("queue pool connects");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM apalis.jobs WHERE job ->> 'name' = 'health_check'")
            .fetch_one(&pool)
            .await
            .expect("count queued jobs");
    assert!(
        count >= 1,
        "expected at least one queued health_check job, got {count}"
    );
}
