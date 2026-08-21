use chrono::Utc;
use prospecting_agent::db;
use prospecting_agent::domain::{
    Channel, Company, Contact, ContactSource, ContactStatus, Engagement, EngagementKind, SequenceState,
    Signal, SignalKind, SignalStrength, StopReason,
};

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");
    Some(pool)
}

macro_rules! require_pool {
    () => {
        match test_pool().await {
            Some(pool) => pool,
            None => {
                eprintln!("TEST_DATABASE_URL not set; skipping db integration test");
                return;
            }
        }
    };
}

#[tokio::test]
async fn contact_upsert_roundtrip_and_status_update() {
    let pool = require_pool!();
    let mut contact = Contact::new(ContactSource::Csv);
    contact.email = Some(format!("it-{}@example.com", contact.id));
    contact.first_name = Some("Test".into());
    db::contacts::upsert(&pool, &contact).await.unwrap();
    let loaded = db::contacts::by_email(&pool, contact.email.as_ref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.id, contact.id);
    assert_eq!(loaded.status, ContactStatus::New);
    db::contacts::set_status(&pool, contact.id, ContactStatus::Enriched)
        .await
        .unwrap();
    let reloaded = db::contacts::by_id(&pool, contact.id).await.unwrap().unwrap();
    assert_eq!(reloaded.status, ContactStatus::Enriched);
    contact.title = Some("VP Sales".into());
    db::contacts::upsert(&pool, &contact).await.unwrap();
    let after = db::contacts::by_id(&pool, contact.id).await.unwrap().unwrap();
    assert_eq!(after.title.as_deref(), Some("VP Sales"));
}

#[tokio::test]
async fn company_upsert_is_idempotent_by_domain() {
    let pool = require_pool!();
    let mut company = Company::new(format!("it-{}.example.com", uuid::Uuid::new_v4()));
    db::companies::upsert(&pool, &company).await.unwrap();
    company.name = Some("Acme".into());
    company.employee_count = Some(250);
    db::companies::upsert(&pool, &company).await.unwrap();
    let loaded = db::companies::by_domain(&pool, &company.domain)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.name.as_deref(), Some("Acme"));
    assert_eq!(loaded.employee_count, Some(250));
}

#[tokio::test]
async fn engagement_and_sequence_state_persist() {
    let pool = require_pool!();
    let contact = Contact::new(ContactSource::Apollo);
    db::contacts::upsert(&pool, &contact).await.unwrap();
    let mut sent = Engagement::outbound(contact.id, Channel::Email, EngagementKind::Sent);
    sent.subject = Some("hello".into());
    sent.sequence_step = Some(1);
    db::engagements::insert(&pool, &sent).await.unwrap();
    let history = db::engagements::for_contact(&pool, contact.id, 10).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].subject.as_deref(), Some("hello"));

    let mut state = SequenceState::start(contact.id, "standard", 3);
    state.advance();
    state.stop(StopReason::Replied);
    db::sequences::upsert(&pool, &state).await.unwrap();
    let loaded = db::sequences::for_contact(&pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.stop_reason, Some(StopReason::Replied));
    assert_eq!(loaded.current_step, 1);
}

#[tokio::test]
async fn signals_store_and_query_by_domain() {
    let pool = require_pool!();
    let domain = format!("sig-{}.example.com", uuid::Uuid::new_v4());
    let s = Signal::new(
        &domain,
        SignalKind::Funding,
        SignalStrength::Strong,
        "raised series B",
    );
    db::signals::insert(&pool, &s).await.unwrap();
    let found = db::signals::for_domain(&pool, &domain, 5).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].strength, SignalStrength::Strong);
}

#[tokio::test]
async fn audit_records_property_changes() {
    let pool = require_pool!();
    let entity_id = uuid::Uuid::new_v4().to_string();
    let change = db::audit::PropertyChange {
        entity_type: "contact".into(),
        entity_id: entity_id.clone(),
        property: "score".into(),
        old_value: Some(serde_json::json!(40)),
        new_value: serde_json::json!(75),
        confidence: Some(0.9),
        updated_by: "signal-detection".into(),
    };
    db::audit::record(&pool, &change).await.unwrap();
    let history = db::audit::history(&pool, "contact", &entity_id, 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].change.new_value, serde_json::json!(75));
    assert_eq!(history[0].change.updated_by, "signal-detection");
}

#[tokio::test]
async fn capacity_reservation_enforces_daily_cap() {
    let pool = require_pool!();
    let sender = format!("cap-{}", uuid::Uuid::new_v4());
    let day = Utc::now().date_naive();
    assert!(db::capacity::try_reserve(&pool, &sender, day, 2).await.unwrap());
    assert!(db::capacity::try_reserve(&pool, &sender, day, 2).await.unwrap());
    assert!(!db::capacity::try_reserve(&pool, &sender, day, 2).await.unwrap());
    assert_eq!(db::capacity::sent_today(&pool, &sender, day).await.unwrap(), 2);
    assert!(!db::capacity::try_reserve(&pool, &sender, day, 0).await.unwrap());
}
