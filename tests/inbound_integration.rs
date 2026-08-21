use prospecting_agent::config::{AppConfig, Secret};
use prospecting_agent::connectors::gmail::{GmailConnector, OauthClient, SenderAccount};
use prospecting_agent::db;
use prospecting_agent::domain::{Contact, ContactSource, ContactStatus};
use prospecting_agent::jobs::JobContext;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

async fn test_ctx() -> Option<JobContext> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so inbound tests cannot pass vacuously"
        );
        eprintln!("TEST_DATABASE_URL not set; skipping inbound integration test");
        return None;
    };
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");
    Some(JobContext::from_config(pool, test_config(&url)))
}

async fn mount_token(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token": "at-1", "expires_in": 3599})),
        )
        .mount(server)
        .await;
}

fn mock_gmail(server: &MockServer, pool: sqlx::PgPool, sender_email: &str) -> GmailConnector {
    let oauth = OauthClient::new("cid", Secret::new("csec")).with_bases(&server.uri(), &server.uri());
    let sender = SenderAccount {
        email: sender_email.to_string(),
        name: "Test Sender".into(),
        refresh_token: "rt-test".into(),
        daily_limit: 50,
    };
    GmailConnector::new(oauth, vec![sender], pool).with_api_base(&server.uri())
}

async fn mount_memory_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m"})))
        .mount(server)
        .await;
}

fn completion_with(content: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"choices": [{"message": {"role": "assistant",
        "content": content.to_string()}}]})
}

#[tokio::test]
async fn reply_monitor_processes_a_matched_reply_once() {
    let Some(mut ctx) = test_ctx().await else { return };
    let gmail_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_token(&gmail_server).await;
    mount_memory_ok(&memory_server).await;

    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let contact_email = format!("replier-{stamp}@inbound.example.com");
    let mut contact = Contact::new(ContactSource::Csv);
    contact.email = Some(contact_email.clone());
    contact.status = ContactStatus::InSequence;
    db::contacts::upsert(&ctx.pool, &contact).await.unwrap();

    let message_id = format!("msg-{stamp}");
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": message_id, "threadId": "t-1"}],
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/gmail/v1/users/me/messages/{message_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": message_id, "threadId": "t-1",
            "snippet": "Sounds interesting, tell me more about pricing.",
            "payload": {"headers": [
                {"name": "From", "value": format!("Replier <{contact_email}>")},
                {"name": "Subject", "value": "Re: Rollout speed"},
            ]},
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "intent": "interested",
                "summary": "Wants pricing details.",
                "suggested_action": "send pricing overview",
                "notify_rep": true,
            }))),
        )
        .mount(&llm_server)
        .await;

    ctx.gmail = Some(mock_gmail(
        &gmail_server,
        ctx.pool.clone(),
        "sender@inbound.example.com",
    ));
    ctx.llm = prospecting_agent::llm::LlmClient::new(&prospecting_agent::config::LlmConfig {
        base_url: format!("{}/v1", llm_server.uri()),
        api_key: Secret::new("test"),
        model: "test-model".into(),
    });
    ctx.memory = prospecting_agent::memory::MemoryClient::new(&prospecting_agent::config::MemoryConfig {
        base_url: memory_server.uri(),
        user: "test".into(),
    });

    let first = prospecting_agent::jobs::inbound::poll_replies(&ctx)
        .await
        .unwrap();
    assert_eq!(first.replies_processed, 1, "report: {first:?}");
    assert_eq!(first.failures, 0);
    let after = db::contacts::by_email(&ctx.pool, &contact_email)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.status, ContactStatus::Replied);
    let engagements = db::engagements::for_contact(&ctx.pool, contact.id, 10)
        .await
        .unwrap();
    assert_eq!(engagements.len(), 1);

    let second = prospecting_agent::jobs::inbound::poll_replies(&ctx)
        .await
        .unwrap();
    assert_eq!(second.replies_processed, 0);
    assert_eq!(second.already_processed, 1);
}

#[tokio::test]
async fn unmatched_sender_is_recorded_but_not_analyzed() {
    let Some(mut ctx) = test_ctx().await else { return };
    let gmail_server = MockServer::start().await;
    mount_token(&gmail_server).await;
    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let message_id = format!("stranger-{stamp}");
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": message_id, "threadId": "t-2"}],
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/gmail/v1/users/me/messages/{message_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": message_id, "threadId": "t-2", "snippet": "hello",
            "payload": {"headers": [
                {"name": "From", "value": format!("Stranger <nobody-{stamp}@unknown.example.com>")},
            ]},
        })))
        .mount(&gmail_server)
        .await;
    ctx.gmail = Some(mock_gmail(
        &gmail_server,
        ctx.pool.clone(),
        "sender@inbound.example.com",
    ));
    let report = prospecting_agent::jobs::inbound::poll_replies(&ctx)
        .await
        .unwrap();
    assert_eq!(report.unknown_sender, 1);
    assert_eq!(report.replies_processed, 0);
}

#[tokio::test]
async fn failed_processing_keeps_the_claim_for_lease_backoff() {
    let Some(mut ctx) = test_ctx().await else { return };
    let gmail_server = MockServer::start().await;
    mount_token(&gmail_server).await;
    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let message_id = format!("broken-{stamp}");
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": message_id, "threadId": "t-3"}],
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/gmail/v1/users/me/messages/{message_id}")))
        .respond_with(ResponseTemplate::new(500))
        .mount(&gmail_server)
        .await;
    ctx.gmail = Some(mock_gmail(
        &gmail_server,
        ctx.pool.clone(),
        "sender@inbound.example.com",
    ));
    let report = prospecting_agent::jobs::inbound::poll_replies(&ctx)
        .await
        .unwrap();
    assert_eq!(report.failures, 1);
    assert_eq!(report.gave_up, 0);
    assert!(
        db::inbound::claim_message(&ctx.pool, &message_id, "sender@inbound.example.com")
            .await
            .unwrap()
            .is_none(),
        "failed message must wait out the lease, not retry immediately"
    );
    sqlx::query(
        "UPDATE processed_inbound SET processed_at = now() - interval '2 hours' WHERE gmail_message_id = $1",
    )
    .bind(&message_id)
    .execute(&ctx.pool)
    .await
    .unwrap();
    let attempts = db::inbound::claim_message(&ctx.pool, &message_id, "sender@inbound.example.com")
        .await
        .unwrap()
        .expect("stale failed claim reopens");
    assert_eq!(attempts, 2);
}

#[tokio::test]
async fn message_claims_are_exclusive() {
    let Some(ctx) = test_ctx().await else { return };
    let id = format!("claim-{}", uuid::Uuid::new_v4().simple());
    assert_eq!(
        db::inbound::claim_message(&ctx.pool, &id, "a@b.c").await.unwrap(),
        Some(1)
    );
    assert!(
        db::inbound::claim_message(&ctx.pool, &id, "a@b.c")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reply_from_a_differently_cased_address_still_matches() {
    let Some(mut ctx) = test_ctx().await else { return };
    let gmail_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_token(&gmail_server).await;
    mount_memory_ok(&memory_server).await;
    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let contact_email = format!("cased-{stamp}@inbound.example.com");
    let mut contact = Contact::new(ContactSource::Csv);
    contact.email = Some(contact_email.clone());
    contact.status = ContactStatus::InSequence;
    db::contacts::upsert(&ctx.pool, &contact).await.unwrap();
    let message_id = format!("cased-{stamp}");
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": message_id, "threadId": "t-9"}],
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/gmail/v1/users/me/messages/{message_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": message_id, "threadId": "t-9",
            "snippet": "Happy to talk.",
            "payload": {"headers": [
                {"name": "From", "value": format!("Cased <{}>", contact_email.to_uppercase())},
            ]},
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "intent": "interested",
                "summary": "Wants to talk.",
                "suggested_action": "book a call",
                "notify_rep": true,
            }))),
        )
        .mount(&llm_server)
        .await;
    ctx.gmail = Some(mock_gmail(
        &gmail_server,
        ctx.pool.clone(),
        "sender@inbound.example.com",
    ));
    ctx.llm = prospecting_agent::llm::LlmClient::new(&prospecting_agent::config::LlmConfig {
        base_url: format!("{}/v1", llm_server.uri()),
        api_key: Secret::new("test"),
        model: "test-model".into(),
    });
    ctx.memory = prospecting_agent::memory::MemoryClient::new(&prospecting_agent::config::MemoryConfig {
        base_url: memory_server.uri(),
        user: "test".into(),
    });
    let report = prospecting_agent::jobs::inbound::poll_replies(&ctx)
        .await
        .unwrap();
    assert_eq!(report.replies_processed, 1);
    assert_eq!(report.unknown_sender, 0);
    let after = db::contacts::by_email(&ctx.pool, &contact_email)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.status, ContactStatus::Replied);
}

#[tokio::test]
async fn polling_follows_page_tokens_across_pages() {
    let Some(mut ctx) = test_ctx().await else { return };
    let gmail_server = MockServer::start().await;
    mount_token(&gmail_server).await;
    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let first_id = format!("page1-{stamp}");
    let second_id = format!("page2-{stamp}");
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .and(wiremock::matchers::query_param("pageToken", "next-page"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": second_id, "threadId": "t-p2"}],
        })))
        .mount(&gmail_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": first_id, "threadId": "t-p1"}],
            "nextPageToken": "next-page",
        })))
        .mount(&gmail_server)
        .await;
    for id in [&first_id, &second_id] {
        Mock::given(method("GET"))
            .and(path(format!("/gmail/v1/users/me/messages/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": id, "threadId": "t", "snippet": "hi",
                "payload": {"headers": [
                    {"name": "From", "value": format!("S <none-{stamp}@unknown.example.com>")},
                ]},
            })))
            .mount(&gmail_server)
            .await;
    }
    ctx.gmail = Some(mock_gmail(
        &gmail_server,
        ctx.pool.clone(),
        "sender@inbound.example.com",
    ));
    let report = prospecting_agent::jobs::inbound::poll_replies(&ctx)
        .await
        .unwrap();
    assert_eq!(report.pages_listed, 2);
    assert_eq!(report.messages_listed, 2);
    assert_eq!(report.unknown_sender, 2);
}

#[tokio::test]
async fn stale_incomplete_claims_are_reclaimed_after_the_lease() {
    let Some(ctx) = test_ctx().await else { return };
    let id = format!("lease-{}", uuid::Uuid::new_v4().simple());
    assert!(
        db::inbound::claim_message(&ctx.pool, &id, "a@b.c")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        db::inbound::claim_message(&ctx.pool, &id, "a@b.c")
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query(
        "UPDATE processed_inbound SET processed_at = now() - interval '2 hours' WHERE gmail_message_id = $1",
    )
    .bind(&id)
    .execute(&ctx.pool)
    .await
    .unwrap();
    assert!(
        db::inbound::claim_message(&ctx.pool, &id, "a@b.c")
            .await
            .unwrap()
            .is_some()
    );
    db::inbound::mark_completed(&ctx.pool, &id).await.unwrap();
    sqlx::query(
        "UPDATE processed_inbound SET processed_at = now() - interval '2 hours' WHERE gmail_message_id = $1",
    )
    .bind(&id)
    .execute(&ctx.pool)
    .await
    .unwrap();
    assert!(
        db::inbound::claim_message(&ctx.pool, &id, "a@b.c")
            .await
            .unwrap()
            .is_none(),
        "completed messages must never be reclaimed"
    );
}
