use chrono::{Duration, Utc};
use prospecting_agent::config::AppConfig;
use prospecting_agent::db;
use prospecting_agent::domain::{AgentTask, TaskKind, TaskStatus};
use prospecting_agent::jobs::JobContext;

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

static EXEC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn test_ctx() -> Option<JobContext> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so task tests cannot pass vacuously"
        );
        eprintln!("TEST_DATABASE_URL not set; skipping task executor test");
        return None;
    };
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");
    Some(JobContext::from_config(pool, test_config(&url)))
}

#[tokio::test]
async fn notify_task_completes_and_future_tasks_stay_queued() {
    let Some(ctx) = test_ctx().await else { return };
    let _exec = EXEC_LOCK.lock().await;
    let mut due_now = AgentTask::new(TaskKind::NotifyRep, serde_json::json!({"message": "ping"}));
    due_now.due_at = None;
    db::tasks::insert(&ctx.pool, &due_now).await.unwrap();
    let mut future = AgentTask::new(TaskKind::NotifyRep, serde_json::json!({"message": "later"}));
    future.due_at = Some(Utc::now() + Duration::hours(6));
    db::tasks::insert(&ctx.pool, &future).await.unwrap();

    prospecting_agent::jobs::tasks::execute_due(&ctx).await.unwrap();

    let done = db::tasks::by_id(&ctx.pool, due_now.id).await.unwrap().unwrap();
    assert_eq!(done.status, TaskStatus::Done);
    assert_eq!(done.attempts, 1);
    let queued = db::tasks::by_id(&ctx.pool, future.id).await.unwrap().unwrap();
    assert_eq!(queued.status, TaskStatus::Pending);
    assert_eq!(queued.attempts, 0);
}

#[tokio::test]
async fn unsupported_kinds_are_skipped_not_failed() {
    let Some(ctx) = test_ctx().await else { return };
    let _exec = EXEC_LOCK.lock().await;
    let task = AgentTask::new(TaskKind::SyncCrm, serde_json::json!({}));
    db::tasks::insert(&ctx.pool, &task).await.unwrap();
    prospecting_agent::jobs::tasks::execute_due(&ctx).await.unwrap();
    let after = db::tasks::by_id(&ctx.pool, task.id).await.unwrap().unwrap();
    assert_eq!(after.status, TaskStatus::Skipped);
}

#[tokio::test]
async fn failing_task_retries_then_fails_permanently() {
    let Some(ctx) = test_ctx().await else { return };
    let _exec = EXEC_LOCK.lock().await;
    let mut task = AgentTask::new(TaskKind::ResearchCompany, serde_json::json!({}));
    task.company_domain = Some(format!("missing-{}.example.com", uuid::Uuid::new_v4().simple()));
    db::tasks::insert(&ctx.pool, &task).await.unwrap();

    prospecting_agent::jobs::tasks::execute_due(&ctx).await.unwrap();
    let retried = db::tasks::by_id(&ctx.pool, task.id).await.unwrap().unwrap();
    assert_eq!(retried.status, TaskStatus::Pending);
    assert_eq!(retried.attempts, 1);
    assert!(retried.due_at.unwrap() > Utc::now());

    let mut exhausted = AgentTask::new(TaskKind::ResearchCompany, serde_json::json!({}));
    exhausted.company_domain = Some(format!("gone-{}.example.com", uuid::Uuid::new_v4().simple()));
    exhausted.attempts = 3;
    db::tasks::insert(&ctx.pool, &exhausted).await.unwrap();
    prospecting_agent::jobs::tasks::execute_due(&ctx).await.unwrap();
    let failed = db::tasks::by_id(&ctx.pool, exhausted.id).await.unwrap().unwrap();
    assert_eq!(failed.status, TaskStatus::Failed);
}

#[tokio::test]
async fn stale_running_tasks_are_reclaimed_and_concurrent_claims_stay_disjoint() {
    let Some(ctx) = test_ctx().await else { return };
    let _exec = EXEC_LOCK.lock().await;
    let task = AgentTask::new(TaskKind::NotifyRep, serde_json::json!({"message": "stale"}));
    db::tasks::insert(&ctx.pool, &task).await.unwrap();
    let first = db::tasks::claim_due(&ctx.pool, Utc::now(), 100_000)
        .await
        .unwrap();
    assert!(first.iter().any(|claimed| claimed.id == task.id));
    let fresh = db::tasks::claim_due(&ctx.pool, Utc::now(), 100_000)
        .await
        .unwrap();
    assert!(!fresh.iter().any(|claimed| claimed.id == task.id));
    sqlx::query("UPDATE agent_tasks SET claimed_at = now() - interval '2 hours' WHERE id = $1")
        .bind(task.id)
        .execute(&ctx.pool)
        .await
        .unwrap();
    let reclaimed = db::tasks::claim_due(&ctx.pool, Utc::now(), 100_000)
        .await
        .unwrap();
    assert!(reclaimed.iter().any(|claimed| claimed.id == task.id));
    db::tasks::finish(&ctx.pool, task.id, TaskStatus::Done)
        .await
        .unwrap();

    let a = AgentTask::new(TaskKind::NotifyRep, serde_json::json!({"message": "a"}));
    let b = AgentTask::new(TaskKind::NotifyRep, serde_json::json!({"message": "b"}));
    db::tasks::insert(&ctx.pool, &a).await.unwrap();
    db::tasks::insert(&ctx.pool, &b).await.unwrap();
    let (left, right) = tokio::join!(
        db::tasks::claim_due(&ctx.pool, Utc::now(), 1),
        db::tasks::claim_due(&ctx.pool, Utc::now(), 1),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    for claimed in &left {
        assert!(!right.iter().any(|other| other.id == claimed.id));
    }
}
