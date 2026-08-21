use std::sync::Arc;

use prospecting_agent::config::AppConfig;
use prospecting_agent::db;
use prospecting_agent::http::webhooks::WebhookRegistry;
use prospecting_agent::http::{AppState, router};
use prospecting_agent::jobs::{JobContext, JobKind};

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
    let queue = prospecting_agent::jobs::runtime::storage(&url)
        .await
        .expect("queue storage");
    let state = Arc::new(AppState::new(ctx, queue, webhooks));
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
    let expected: Vec<&str> = JobKind::ALL.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(jobs, expected);
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
        Err(prospecting_agent::http::webhooks::WebhookError::Rejected(
            "boom".to_string(),
        ))
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
async fn queue_enqueue_persists_exactly_one_new_job_row() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so queue tests cannot pass vacuously"
        );
        eprintln!("TEST_DATABASE_URL not set; skipping queue integration test");
        return;
    };
    let backend = prospecting_agent::jobs::runtime::storage(&url)
        .await
        .expect("queue storage");
    let pool = prospecting_agent::jobs::runtime::queue_pool(&url)
        .await
        .expect("queue pool connects");
    let count = |pool: sqlx::PgPool| async move {
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM apalis.jobs WHERE job ->> 'name' = 'health_check'")
                .fetch_one(&pool)
                .await
                .expect("count queued jobs");
        n
    };
    let before = count(pool.clone()).await;
    prospecting_agent::jobs::runtime::enqueue(&backend, JobKind::HealthCheck)
        .await
        .expect("enqueue succeeds");
    let after = count(pool).await;
    assert_eq!(after, before + 1);
}

#[tokio::test]
async fn queue_schemas_stay_isolated_from_the_app_schema() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so isolation tests cannot pass vacuously"
        );
        eprintln!("TEST_DATABASE_URL not set; skipping schema isolation test");
        return;
    };
    let app_pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&app_pool).await.expect("run app migrations");
    prospecting_agent::jobs::runtime::storage(&url)
        .await
        .expect("queue storage setup");
    let exists = |pool: sqlx::PgPool, relation: &'static str| async move {
        let found: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(relation)
            .fetch_one(&pool)
            .await
            .expect("to_regclass lookup");
        found.is_some()
    };
    assert!(exists(app_pool.clone(), "public._sqlx_migrations").await);
    assert!(exists(app_pool.clone(), "apalis_meta._sqlx_migrations").await);
    assert!(!exists(app_pool.clone(), "public.jobs").await);
    assert!(!exists(app_pool.clone(), "public.workers").await);
    let stray_ulid: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace          WHERE p.proname = 'generate_ulid' AND n.nspname = 'public'",
    )
    .fetch_one(&app_pool)
    .await
    .expect("pg_proc lookup");
    assert_eq!(stray_ulid, 0);
}

#[tokio::test]
async fn health_endpoint_reports_service_unavailable_when_backends_are_down() {
    let base = require_server!(WebhookRegistry::default());
    let response = reqwest::get(format!("{base}/health")).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], "error");
}

#[tokio::test]
async fn run_job_now_health_check_returns_a_report() {
    let base = require_server!(WebhookRegistry::default());
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base}/control/jobs/health_check/run"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["checks"].is_array());
}

#[tokio::test]
async fn enqueue_endpoint_accepts_a_known_job() {
    let base = require_server!(WebhookRegistry::default());
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base}/control/jobs/weekly_report/enqueue"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["enqueued"], "weekly_report");
}

#[tokio::test]
async fn contact_lookup_returns_the_seeded_contact() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL not set; skipping http integration test");
        return;
    };
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");
    let email = format!("lookup-{}@ctl.example.com", uuid::Uuid::new_v4().simple());
    let mut contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    contact.email = Some(email.clone());
    prospecting_agent::db::contacts::upsert(&pool, &contact)
        .await
        .expect("seed contact");
    let base = require_server!(WebhookRegistry::default());
    let body: serde_json::Value = reqwest::get(format!("{base}/control/contacts/{email}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["contact"]["email"], serde_json::json!(email));
    assert!(body["engagements"].is_array());
}
