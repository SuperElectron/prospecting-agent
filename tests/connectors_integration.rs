use prospecting_agent::config::Secret;
use prospecting_agent::connectors::gmail::auth::SenderAccount;
use prospecting_agent::connectors::gmail::{GmailConnector, OauthClient};
use prospecting_agent::connectors::{ConnectorError, EmailTransport, OutboundEmail, ThreadContext};
use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so connector tests cannot pass vacuously"
        );
        return None;
    };
    let pool = prospecting_agent::db::connect(&url).await.expect("connect");
    prospecting_agent::db::migrate(&pool).await.expect("migrate");
    Some(pool)
}

macro_rules! require_pool {
    () => {
        match test_pool().await {
            Some(pool) => pool,
            None => {
                eprintln!("TEST_DATABASE_URL not set; skipping connector integration test");
                return;
            }
        }
    };
}

fn sender(email: &str, limit: i32) -> SenderAccount {
    SenderAccount {
        email: email.into(),
        name: email.split('@').next().unwrap_or("s").into(),
        refresh_token: format!("rt-{email}"),
        daily_limit: limit,
    }
}

fn oauth(server: &MockServer) -> OauthClient {
    OauthClient::new("cid", Secret::new("csec")).with_bases(&server.uri(), &server.uri())
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

async fn mount_send(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/gmail/v1/users/me/messages/send"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m-1", "threadId": "t-1"})),
        )
        .mount(server)
        .await;
}

fn unique_sender(prefix: &str) -> String {
    format!("{prefix}-{}@ours.io", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn send_produces_receipt_and_consumes_capacity() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_token(&server).await;
    mount_send(&server).await;
    let account = sender(&unique_sender("send"), 5);
    let connector =
        GmailConnector::new(oauth(&server), vec![account.clone()], pool.clone()).with_api_base(&server.uri());
    let receipt = connector
        .send(&OutboundEmail::new(
            "jane@acme.io",
            "Quick question",
            "plain body",
        ))
        .await
        .unwrap();
    assert_eq!(receipt.message_id, "m-1");
    assert_eq!(receipt.thread_id, "t-1");
    assert_eq!(receipt.sender_email, account.email);
    let day = chrono::Utc::now().date_naive();
    let sent = prospecting_agent::db::capacity::sent_today(&pool, &account.email, day)
        .await
        .unwrap();
    assert_eq!(sent, 1);
}

#[tokio::test]
async fn exhausted_capacity_rotates_to_the_next_sender_then_errors() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_token(&server).await;
    mount_send(&server).await;
    let first = sender(&unique_sender("cap1"), 1);
    let second = sender(&unique_sender("cap2"), 1);
    let connector = GmailConnector::new(oauth(&server), vec![first.clone(), second.clone()], pool.clone())
        .with_api_base(&server.uri());
    let email = OutboundEmail::new("jane@acme.io", "s", "b");
    let first_receipt = connector.send(&email).await.unwrap();
    let second_receipt = connector.send(&email).await.unwrap();
    assert_ne!(first_receipt.sender_email, second_receipt.sender_email);
    let err = connector.send(&email).await.unwrap_err();
    assert!(matches!(err, ConnectorError::NoCapacity { senders: 2 }));
}

#[tokio::test]
async fn reply_threads_through_preferred_sender_with_thread_headers() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/gmail/v1/users/me/messages/send"))
        .and(body_partial_json(serde_json::json!({"threadId": "t-9"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m-2", "threadId": "t-9"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let preferred = sender(&unique_sender("pref"), 5);
    let other = sender(&unique_sender("other"), 5);
    let connector = GmailConnector::new(oauth(&server), vec![other, preferred.clone()], pool.clone())
        .with_api_base(&server.uri());
    let email = OutboundEmail {
        to: "jane@acme.io".into(),
        subject: "original".into(),
        body_text: "reply body".into(),
        body_html: None,
        thread: Some(ThreadContext {
            thread_id: "t-9".into(),
            in_reply_to: Some("<m1@mail>".into()),
            references: vec!["<m1@mail>".into()],
            original_subject: Some("Hello there".into()),
            prefer_sender: Some(preferred.email.clone()),
        }),
    };
    let receipt = connector.send(&email).await.unwrap();
    assert_eq!(receipt.sender_email, preferred.email);
    assert_eq!(receipt.thread_id, "t-9");
}

#[tokio::test]
async fn invalid_recipient_fails_before_any_network_or_capacity_use() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    let account = sender(&unique_sender("inv"), 5);
    let connector =
        GmailConnector::new(oauth(&server), vec![account.clone()], pool.clone()).with_api_base(&server.uri());
    let err = connector
        .send(&OutboundEmail::new("not-an-email", "s", "b"))
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::InvalidRecipient(_)));
    let day = chrono::Utc::now().date_naive();
    let sent = prospecting_agent::db::capacity::sent_today(&pool, &account.email, day)
        .await
        .unwrap();
    assert_eq!(sent, 0);
}

#[tokio::test]
async fn gmail_api_error_is_typed_with_status() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/gmail/v1/users/me/messages/send"))
        .respond_with(ResponseTemplate::new(403).set_body_string("rate limited"))
        .mount(&server)
        .await;
    let connector = GmailConnector::new(
        oauth(&server),
        vec![sender(&unique_sender("err"), 5)],
        pool.clone(),
    )
    .with_api_base(&server.uri());
    let err = connector
        .send(&OutboundEmail::new("jane@acme.io", "s", "b"))
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::Status { status: 403, .. }));
}

#[tokio::test]
async fn poll_lists_and_fetches_message_metadata() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": "in-1", "threadId": "t-1"}],
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages/in-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "in-1", "threadId": "t-1", "snippet": "Thanks, interested!",
            "payload": {"headers": [
                {"name": "From", "value": "Jane <jane@acme.io>"},
                {"name": "Subject", "value": "Re: Quick question"},
                {"name": "Message-ID", "value": "<abc@mail.gmail.com>"},
                {"name": "References", "value": "<m1@mail> <m2@mail>"},
            ]},
        })))
        .expect(1)
        .mount(&server)
        .await;
    let account = sender(&unique_sender("poll"), 5);
    let connector =
        GmailConnector::new(oauth(&server), vec![account.clone()], pool).with_api_base(&server.uri());
    let refs = connector
        .list_messages(&account.email, "in:inbox newer_than:1d", 10)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    let message = connector
        .fetch_message(&account.email, &refs[0].id)
        .await
        .unwrap();
    assert_eq!(message.from.as_deref(), Some("Jane <jane@acme.io>"));
    assert_eq!(message.message_id_header.as_deref(), Some("<abc@mail.gmail.com>"));
    assert_eq!(message.references, vec!["<m1@mail>", "<m2@mail>"]);
    assert_eq!(message.snippet, "Thanks, interested!");
}

#[tokio::test]
async fn token_exchange_failure_surfaces_as_auth_chain() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#))
        .mount(&server)
        .await;
    let connector = GmailConnector::new(oauth(&server), vec![sender(&unique_sender("badtok"), 5)], pool)
        .with_api_base(&server.uri());
    let err = connector
        .send(&OutboundEmail::new("jane@acme.io", "s", "b"))
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectorError::Status { status: 400, .. }));
}

#[tokio::test]
async fn live_gmail_self_send_and_poll_smoke() {
    if std::env::var("TEST_GMAIL_LIVE").is_err() {
        eprintln!("TEST_GMAIL_LIVE not set; skipping live gmail smoke");
        return;
    }
    let pool = require_pool!();
    let client_file = std::env::var("GMAIL_CLIENT_FILE")
        .unwrap_or_else(|_| ".claude/secrets/gmail-oauth-client.json".into());
    let senders_file =
        std::env::var("GMAIL_SENDERS_FILE").unwrap_or_else(|_| ".claude/secrets/gmail-senders.json".into());
    let oauth = prospecting_agent::connectors::gmail::OauthClient::from_file(&client_file).unwrap();
    let senders = prospecting_agent::connectors::gmail::load_senders(&senders_file).unwrap();
    assert!(!senders.is_empty(), "run cli gmail-auth first");
    let self_address = senders[0].email.clone();
    let connector = GmailConnector::new(oauth, senders, pool);
    let receipt = connector
        .send(&OutboundEmail::new(
            &self_address,
            "prospecting-agent live smoke",
            "self-addressed smoke test from the gmail connector",
        ))
        .await
        .unwrap();
    assert!(!receipt.message_id.is_empty());
    let refs = connector
        .list_messages(
            &self_address,
            "newer_than:1d subject:(prospecting-agent live smoke)",
            5,
        )
        .await
        .unwrap();
    assert!(!refs.is_empty());
    let message = connector.fetch_message(&self_address, &refs[0].id).await.unwrap();
    assert_eq!(message.thread_id, receipt.thread_id);
}
