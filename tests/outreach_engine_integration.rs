use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use prospecting_agent::config::{Cadence, MessagingRules, Secret};
use prospecting_agent::connectors::{ConnectorError, EmailTransport, OutboundEmail, SendReceipt};
use prospecting_agent::db;
use prospecting_agent::domain::{Contact, ContactSource, ContactStatus, SequenceState, StopReason};
use prospecting_agent::memory::MemoryClient;
use prospecting_agent::workflows::accounts::PreflightConfig;
use prospecting_agent::workflows::outreach::{SendPassInputs, enroll_contacts, run_send_pass};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so outreach tests cannot pass vacuously"
        );
        return None;
    };
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");
    Some(pool)
}

async fn cross_process_sweep_lock() -> sqlx::PgConnection {
    use sqlx::Connection;
    let url = std::env::var("TEST_DATABASE_URL").expect("guard runs only with a test database");
    let mut conn = sqlx::PgConnection::connect(&url).await.expect("lock connection");
    sqlx::query("SELECT pg_advisory_lock(73461122)")
        .execute(&mut conn)
        .await
        .expect("advisory lock");
    conn
}

macro_rules! require_pool {
    () => {
        match test_pool().await {
            Some(pool) => pool,
            None => {
                eprintln!("TEST_DATABASE_URL not set; skipping outreach integration test");
                return;
            }
        }
    };
}

#[derive(Default)]
struct RecordingTransport {
    sent: Mutex<Vec<OutboundEmail>>,
}

impl EmailTransport for RecordingTransport {
    async fn send(&self, email: &OutboundEmail) -> Result<SendReceipt, ConnectorError> {
        self.sent.lock().unwrap().push(email.clone());
        Ok(SendReceipt {
            message_id: "m-1".into(),
            thread_id: "t-1".into(),
            sender_email: "sender@test.example.com".into(),
        })
    }
}

struct FailingTransport;

impl EmailTransport for FailingTransport {
    async fn send(&self, _email: &OutboundEmail) -> Result<SendReceipt, ConnectorError> {
        Err(ConnectorError::NoSenders)
    }
}

fn memory_client(server: &MockServer) -> MemoryClient {
    MemoryClient::new(&prospecting_agent::config::MemoryConfig {
        base_url: server.uri(),
        user: "test".into(),
    })
}

fn llm_client(server: &MockServer) -> prospecting_agent::llm::LlmClient {
    prospecting_agent::llm::LlmClient::new(&prospecting_agent::config::LlmConfig {
        base_url: format!("{}/v1", server.uri()),
        api_key: Secret::new("test"),
        model: "test-model".into(),
    })
}

async fn mount_memory_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m"})))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/filter"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [], "total": 0, "page": 1, "size": 50, "pages": 0,
        })))
        .mount(server)
        .await;
}

async fn mount_llm_email(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": serde_json::json!({
                "subject": "Rollout speed",
                "body": "Hi there,\n\nSaw the billing API work. Worth a quick chat about rollout speed?\n\nBest",
                "personalization_fact": "they build billing APIs",
            }).to_string()}}],
        })))
        .mount(server)
        .await;
}

async fn seed_contact(pool: &sqlx::PgPool, status: ContactStatus) -> Contact {
    let mut contact = Contact::new(ContactSource::Csv);
    contact.email = Some(format!(
        "eng-{}@engine.example.com",
        uuid::Uuid::new_v4().simple()
    ));
    contact.status = status;
    db::contacts::upsert(pool, &contact).await.unwrap();
    contact
}

fn tuesday_morning() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 18, 10, 0, 0).unwrap()
}

fn sunday_morning() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 16, 10, 0, 0).unwrap()
}

fn inputs<'a, T: EmailTransport>(
    memory: &'a MemoryClient,
    llm: &'a prospecting_agent::llm::LlmClient,
    policies: &'a [prospecting_agent::llm::Policy],
    rules: &'a MessagingRules,
    preflight: &'a PreflightConfig,
    transport: Option<&'a T>,
    dry_run: bool,
) -> SendPassInputs<'a, T> {
    SendPassInputs {
        memory,
        llm,
        policies,
        rules,
        preflight_config: preflight,
        transport,
        dry_run,
        limit: 100_000,
    }
}

#[tokio::test]
async fn enrollment_starts_sequences_only_for_contactable_enriched_contacts() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let enriched = seed_contact(&pool, ContactStatus::Enriched).await;
    let opted_out = seed_contact(&pool, ContactStatus::OptedOut).await;
    let cadence = Cadence::standard();
    let report = enroll_contacts(&pool, &cadence, 100_000).await.unwrap();
    assert!(report.enrolled >= 1);
    let state = db::sequences::for_contact(&pool, enriched.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.cadence, "standard");
    assert_eq!(state.current_step, 0);
    let after = db::contacts::by_id(&pool, enriched.id).await.unwrap().unwrap();
    assert_eq!(after.status, ContactStatus::InSequence);
    assert!(
        db::sequences::for_contact(&pool, opted_out.id)
            .await
            .unwrap()
            .is_none()
    );

    let again = enroll_contacts(&pool, &cadence, 100_000).await.unwrap();
    let re_enrolled = db::sequences::for_contact(&pool, enriched.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(re_enrolled.current_step, 0);
    assert!(again.enrolled == 0 || re_enrolled.current_step == 0);
}

#[tokio::test]
async fn send_pass_sends_records_and_advances_inside_the_window() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::InSequence).await;
    let state = SequenceState::start(contact.id, "standard", 3);
    db::sequences::upsert(&pool, &state).await.unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let transport = RecordingTransport::default();
    let report = run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&transport),
            false,
        ),
        tuesday_morning(),
    )
    .await
    .unwrap();
    assert!(report.sent >= 1);
    {
        let sent = transport.sent.lock().unwrap();
        assert!(
            sent.iter()
                .any(|email| email.to == contact.email.clone().unwrap())
        );
    }
    let after = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.current_step, 1);
    assert_eq!(after.last_sent_at, Some(tuesday_morning()));
    let engagements = db::engagements::for_contact(&pool, contact.id, 10).await.unwrap();
    assert_eq!(engagements.len(), 1);
    assert_eq!(engagements[0].subject.as_deref(), Some("Rollout speed"));
    assert_eq!(engagements[0].sequence_step, Some(1));
}

#[tokio::test]
async fn send_pass_outside_the_window_sends_nothing() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::InSequence).await;
    db::sequences::upsert(&pool, &SequenceState::start(contact.id, "standard", 3))
        .await
        .unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let transport = RecordingTransport::default();
    run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&transport),
            false,
        ),
        sunday_morning(),
    )
    .await
    .unwrap();
    assert!(
        db::sequences::for_contact(&pool, contact.id)
            .await
            .unwrap()
            .unwrap()
            .current_step
            == 0
    );
    assert!(
        !transport
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|email| email.to == contact.email.clone().unwrap())
    );
}

#[tokio::test]
async fn dry_run_drafts_without_side_effects() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::InSequence).await;
    db::sequences::upsert(&pool, &SequenceState::start(contact.id, "standard", 3))
        .await
        .unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let report = run_send_pass(
        &pool,
        &inputs::<RecordingTransport>(&memory, &llm, &policies, &rules, &preflight, None, true),
        tuesday_morning(),
    )
    .await
    .unwrap();
    assert!(report.drafted_dry_run >= 1);
    let after = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.current_step, 0);
    assert!(
        db::engagements::for_contact(&pool, contact.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn opted_out_contact_stops_the_sequence_before_any_send() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::OptedOut).await;
    db::sequences::upsert(&pool, &SequenceState::start(contact.id, "standard", 3))
        .await
        .unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let transport = RecordingTransport::default();
    run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&transport),
            false,
        ),
        tuesday_morning(),
    )
    .await
    .unwrap();
    let state = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert!(state.stopped);
    assert_eq!(state.stop_reason, Some(StopReason::OptedOut));
    assert!(state.stopped_at.is_some());
    assert!(
        !transport
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|email| email.to == contact.email.clone().unwrap())
    );
}

#[tokio::test]
async fn follow_up_outside_the_window_waits_even_when_overdue() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::InSequence).await;
    let mut state = SequenceState::start(contact.id, "standard", 3);
    state.current_step = 1;
    state.last_sent_at = Some(Utc.with_ymd_and_hms(2026, 8, 11, 10, 0, 0).unwrap());
    db::sequences::upsert(&pool, &state).await.unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let transport = RecordingTransport::default();
    let report = run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&transport),
            false,
        ),
        sunday_morning(),
    )
    .await
    .unwrap();
    assert!(report.outside_window >= 1);
    let after = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.current_step, 1);
    assert!(
        !transport
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|email| email.to == contact.email.clone().unwrap())
    );
}

#[tokio::test]
async fn preflight_modify_caps_the_sequence_and_is_counted() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let domain = format!("warm-{}.example.com", uuid::Uuid::new_v4().simple());
    let company = prospecting_agent::domain::Company::new(&domain);
    db::companies::upsert(&pool, &company).await.unwrap();
    let mut contact = seed_contact(&pool, ContactStatus::Enriched).await;
    contact.company_domain = Some(domain.clone());
    db::contacts::upsert(&pool, &contact).await.unwrap();
    let strategy = prospecting_agent::domain::AccountStrategy {
        domain: domain.clone(),
        stage: prospecting_agent::domain::AccountStage::Engaged,
        health: prospecting_agent::domain::AccountHealth::Healthy,
        coordination_flags: vec![prospecting_agent::domain::FLAG_NEW_CONTACT_ADVANCED.to_string()],
        summary: "advanced account; warm intro only".into(),
        updated_at: Utc::now(),
    };
    db::strategies::upsert(&pool, &strategy).await.unwrap();
    db::sequences::upsert(&pool, &SequenceState::start(contact.id, "standard", 3))
        .await
        .unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let transport = RecordingTransport::default();
    let report = run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&transport),
            false,
        ),
        tuesday_morning(),
    )
    .await
    .unwrap();
    assert!(report.preflight_modified >= 1);
    let after = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.max_steps, 2);
}

#[tokio::test]
async fn failed_send_burns_the_slot_without_recording_an_engagement() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::InSequence).await;
    db::sequences::upsert(&pool, &SequenceState::start(contact.id, "standard", 3))
        .await
        .unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let report = run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&FailingTransport),
            false,
        ),
        tuesday_morning(),
    )
    .await
    .unwrap();
    assert!(report.send_failed >= 1);
    let state = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_step, 1);
    assert!(!state.stopped);
    assert!(
        db::engagements::for_contact(&pool, contact.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn final_step_send_failure_is_not_recorded_as_a_completion() {
    let pool = require_pool!();
    let _sweep = cross_process_sweep_lock().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_llm_email(&llm_server).await;
    mount_memory_ok(&memory_server).await;
    let contact = seed_contact(&pool, ContactStatus::InSequence).await;
    let mut state = SequenceState::start(contact.id, "standard", 3);
    state.current_step = 2;
    state.last_sent_at = Some(Utc.with_ymd_and_hms(2026, 8, 11, 10, 0, 0).unwrap());
    db::sequences::upsert(&pool, &state).await.unwrap();
    let memory = memory_client(&memory_server);
    let llm = llm_client(&llm_server);
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let preflight = PreflightConfig::default();
    let report = run_send_pass(
        &pool,
        &inputs(
            &memory,
            &llm,
            &policies,
            &rules,
            &preflight,
            Some(&FailingTransport),
            false,
        ),
        tuesday_morning(),
    )
    .await
    .unwrap();
    assert!(report.send_failed >= 1);
    let after = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert!(after.stopped);
    assert_eq!(after.stop_reason, Some(StopReason::Manual));
}
