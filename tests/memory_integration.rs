use prospecting_agent::config::MemoryConfig;
use prospecting_agent::memory::{EntityRef, MemoryClient, MemoryError};

#[tokio::test]
#[ignore = "live smoke against a running memory stack; run with --ignored and TEST_MEMORY_URL"]
async fn live_memory_service_smoke() {
    let Ok(url) = std::env::var("TEST_MEMORY_URL") else {
        eprintln!("TEST_MEMORY_URL not set; skipping memory smoke test");
        return;
    };
    let config = MemoryConfig {
        base_url: url,
        user: std::env::var("TEST_MEMORY_USER").unwrap_or_else(|_| "prospecting".into()),
    };
    let client = MemoryClient::new(&config);
    let entity = EntityRef::company("smoke-test.example.com");

    let recall = client.recall("smoke test", Some(&entity), 3).await;
    assert!(recall.is_ok(), "filter endpoint should respond: {recall:?}");

    if std::env::var("TEST_MEMORY_WRITE").is_ok() {
        match client.memorize(&entity, "smoke test memory entry", false).await {
            Ok(()) => {}
            Err(MemoryError::Backend(reason)) => {
                eprintln!("memorize skipped — backend LLM/embedder not configured: {reason}");
            }
            Err(other) => panic!("unexpected memorize failure: {other}"),
        }
    }
}
