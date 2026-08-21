use prospecting_agent::memory::{EntityRef, MemoryClient, MemoryError};

#[tokio::test]
async fn live_memory_service_smoke() {
    let Ok(url) = std::env::var("TEST_MEMORY_URL") else {
        eprintln!("TEST_MEMORY_URL not set; skipping memory smoke test");
        return;
    };
    let user = std::env::var("TEST_MEMORY_USER").unwrap_or_else(|_| "prospecting".into());
    let client = MemoryClient::new(&url, user);
    let entity = EntityRef::Company("smoke-test.example.com".into());

    let recall = client.recall("smoke test", Some(&entity), 3).await;
    assert!(recall.is_ok(), "filter endpoint should respond: {recall:?}");

    match client.memorize(&entity, "smoke test memory entry", false).await {
        Ok(()) => {}
        Err(MemoryError::Backend(reason)) => {
            eprintln!("memorize skipped — backend LLM/embedder not configured: {reason}");
        }
        Err(other) => panic!("unexpected memorize failure: {other}"),
    }
}
