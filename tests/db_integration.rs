use chrono::Utc;
use prospecting_agent::db;
use prospecting_agent::domain::{
    Channel, Company, Contact, ContactSource, ContactStatus, Engagement, EngagementKind, SequenceState,
    Signal, SignalKind, SignalStrength, StopReason,
};

async fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so db tests cannot pass vacuously"
        );
        return None;
    };
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
async fn contact_reimport_with_new_uuid_updates_by_email() {
    let pool = require_pool!();
    let email = format!("dupe-{}@example.com", uuid::Uuid::new_v4());
    let mut first = Contact::new(ContactSource::Csv);
    first.email = Some(email.clone());
    let persisted_id = db::contacts::upsert(&pool, &first).await.unwrap();
    let mut second = Contact::new(ContactSource::Apollo);
    second.email = Some(email.clone());
    second.title = Some("CRO".into());
    let resolved_id = db::contacts::upsert(&pool, &second).await.unwrap();
    assert_eq!(persisted_id, resolved_id);
    let loaded = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    assert_eq!(loaded.id, persisted_id);
    assert_eq!(loaded.title.as_deref(), Some("CRO"));
}

#[tokio::test]
async fn company_reupsert_returns_the_persisted_id() {
    let pool = require_pool!();
    let domain = format!("idkeep-{}.example.com", uuid::Uuid::new_v4());
    let first = Company::new(domain.clone());
    let persisted_id = db::companies::upsert(&pool, &first).await.unwrap();
    assert_eq!(persisted_id, first.id);
    let second = Company::new(domain.clone());
    let resolved_id = db::companies::upsert(&pool, &second).await.unwrap();
    assert_eq!(resolved_id, persisted_id);
    assert_ne!(resolved_id, second.id);
}

#[tokio::test]
async fn duplicate_engagement_delivery_is_ignored() {
    let pool = require_pool!();
    let contact = Contact::new(ContactSource::Csv);
    db::contacts::upsert(&pool, &contact).await.unwrap();
    let event = Engagement::outbound(contact.id, Channel::Email, EngagementKind::Opened);
    assert!(db::engagements::insert(&pool, &event).await.unwrap());
    let mut redelivered = event.clone();
    redelivered.id = uuid::Uuid::new_v4();
    assert!(!db::engagements::insert(&pool, &redelivered).await.unwrap());
    let history = db::engagements::for_contact(&pool, contact.id, 10).await.unwrap();
    assert_eq!(history.len(), 1);
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
    db::signals::upsert(&pool, &s).await.unwrap();
    let found = db::signals::for_domain(&pool, &domain, 5).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].strength, SignalStrength::Strong);
}

#[tokio::test]
async fn contact_upsert_survives_an_email_change_on_an_existing_id() {
    let pool = require_pool!();
    let mut contact = Contact::new(ContactSource::Csv);
    contact.email = None;
    db::contacts::upsert(&pool, &contact).await.unwrap();
    contact.email = Some(format!("late-{}@example.com", contact.id));
    let resolved = db::contacts::upsert(&pool, &contact).await.unwrap();
    assert_eq!(resolved, contact.id);
    let loaded = db::contacts::by_id(&pool, contact.id).await.unwrap().unwrap();
    assert_eq!(loaded.email, contact.email);
}

#[tokio::test]
async fn signal_lookup_matches_a_normalized_company_domain() {
    let pool = require_pool!();
    let slug = uuid::Uuid::new_v4();
    let company = Company::new(format!("https://www.Norm-{slug}.example.com"));
    db::companies::upsert(&pool, &company).await.unwrap();
    let signal = Signal::new(
        format!("www.Norm-{slug}.example.com"),
        SignalKind::Funding,
        SignalStrength::Strong,
        "raised",
    );
    db::signals::upsert(&pool, &signal).await.unwrap();
    let found = db::signals::for_domain(&pool, &company.domain, 5).await.unwrap();
    assert_eq!(found.len(), 1);
    let via_raw = db::signals::for_domain(&pool, &format!("WWW.norm-{slug}.EXAMPLE.com"), 5)
        .await
        .unwrap();
    assert_eq!(via_raw.len(), 1);
}

#[tokio::test]
async fn repeated_signal_upsert_keeps_one_row_and_refreshes_recency() {
    let pool = require_pool!();
    let domain = format!("dupe-sig-{}.example.com", uuid::Uuid::new_v4());
    let first = Signal::new(
        &domain,
        SignalKind::Hiring,
        SignalStrength::Moderate,
        "hiring ops",
    );
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = Signal::new(&domain, SignalKind::Hiring, SignalStrength::Strong, "hiring ops");
    db::signals::upsert(&pool, &first).await.unwrap();
    db::signals::upsert(&pool, &second).await.unwrap();
    let found = db::signals::for_domain(&pool, &domain, 10).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, first.id);
    assert_eq!(found[0].strength, SignalStrength::Strong);
    assert!(found[0].detected_at > first.detected_at);
}

#[tokio::test]
async fn double_send_at_different_instants_is_deduped() {
    let pool = require_pool!();
    let contact = Contact::new(ContactSource::Csv);
    db::contacts::upsert(&pool, &contact).await.unwrap();
    let mut first = Engagement::outbound(contact.id, Channel::Email, EngagementKind::Sent);
    first.sequence_step = Some(2);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let mut second = Engagement::outbound(contact.id, Channel::Email, EngagementKind::Sent);
    second.sequence_step = Some(2);
    assert_ne!(first.occurred_at, second.occurred_at);
    assert!(db::engagements::insert(&pool, &first).await.unwrap());
    assert!(!db::engagements::insert(&pool, &second).await.unwrap());
    let history = db::engagements::for_contact(&pool, contact.id, 10).await.unwrap();
    assert_eq!(history.len(), 1);
}

#[tokio::test]
async fn employee_count_above_i32_max_is_an_error_not_a_clamp() {
    let pool = require_pool!();
    let mut company = Company::new(format!("big-{}.example.com", uuid::Uuid::new_v4()));
    company.employee_count = Some(u32::MAX);
    let err = db::companies::upsert(&pool, &company).await.unwrap_err();
    assert!(matches!(
        err,
        db::DbError::Codec {
            context: "company.employee_count",
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reservations_never_exceed_the_cap() {
    let pool = require_pool!();
    let sender = format!("conc-{}", uuid::Uuid::new_v4());
    let day = Utc::now().date_naive();
    let mut handles = Vec::new();
    for _ in 0..10 {
        let pool = pool.clone();
        let sender = sender.clone();
        handles.push(tokio::spawn(async move {
            db::capacity::try_reserve(&pool, &sender, day, 3).await
        }));
    }
    let mut granted = 0;
    for handle in handles {
        if handle.await.unwrap().unwrap() {
            granted += 1;
        }
    }
    assert_eq!(granted, 3);
    assert_eq!(db::capacity::sent_today(&pool, &sender, day).await.unwrap(), 3);
}

#[tokio::test]
async fn audit_round_trips_null_confidence_and_null_old_value() {
    let pool = require_pool!();
    let entity_id = uuid::Uuid::new_v4().to_string();
    let change = db::PropertyChange {
        entity_type: "company".into(),
        entity_id: entity_id.clone(),
        property: "summary".into(),
        old_value: None,
        new_value: serde_json::json!("first summary"),
        confidence: None,
        updated_by: "research".into(),
    };
    db::audit::record(&pool, &change).await.unwrap();
    let history = db::audit::history(&pool, "company", &entity_id, 10)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].change.old_value, None);
    assert_eq!(history[0].change.confidence, None);
}

#[tokio::test]
async fn audit_records_property_changes() {
    let pool = require_pool!();
    let entity_id = uuid::Uuid::new_v4().to_string();
    let change = db::PropertyChange {
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

#[tokio::test]
async fn companies_without_contacts_rank_by_icp_score_with_cooldown_and_exclusions() {
    let pool = require_pool!();
    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let lonely_high = format!("lonely-high-{stamp}.example.com");
    let lonely_low = format!("lonely-low-{stamp}.example.com");
    let lonely_unscored = format!("lonely-unscored-{stamp}.example.com");
    let staffed = format!("staffed-{stamp}.example.com");
    let attempted_recently = format!("attempted-{stamp}.example.com");
    for (domain, score) in [
        (&lonely_high, Some(95)),
        (&lonely_low, Some(10)),
        (&lonely_unscored, None),
        (&staffed, Some(99)),
        (&attempted_recently, Some(97)),
    ] {
        let mut company = Company::new(domain);
        company.icp_fit_score = score;
        db::companies::upsert(&pool, &company).await.unwrap();
    }
    let mut staffed_contact = Contact::new(ContactSource::Csv);
    staffed_contact.email = Some(format!("someone@{staffed}"));
    staffed_contact.company_domain = Some(format!("HTTPS://WWW.{}/", staffed.to_uppercase()));
    db::contacts::upsert(&pool, &staffed_contact).await.unwrap();
    let mut homeless_contact = Contact::new(ContactSource::Csv);
    homeless_contact.email = Some(format!("nowhere-{stamp}@example.com"));
    homeless_contact.company_domain = None;
    db::contacts::upsert(&pool, &homeless_contact).await.unwrap();
    db::companies::mark_discovery_attempted(&pool, &attempted_recently)
        .await
        .unwrap();

    let listed = db::companies::list_without_contacts(&pool, 100_000)
        .await
        .unwrap();
    let domains: Vec<&str> = listed.iter().map(|company| company.domain.as_str()).collect();
    assert!(domains.contains(&lonely_high.as_str()));
    assert!(domains.contains(&lonely_low.as_str()));
    assert!(domains.contains(&lonely_unscored.as_str()));
    assert!(!domains.contains(&staffed.as_str()));
    assert!(!domains.contains(&attempted_recently.as_str()));
    let high_pos = domains.iter().position(|d| *d == lonely_high).unwrap();
    let low_pos = domains.iter().position(|d| *d == lonely_low).unwrap();
    let unscored_pos = domains.iter().position(|d| *d == lonely_unscored).unwrap();
    assert!(high_pos < low_pos);
    assert!(low_pos < unscored_pos);
}

#[tokio::test]
async fn discovery_listing_breaks_score_ties_by_oldest_and_honors_limit() {
    let pool = require_pool!();
    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let older = format!("tie-older-{stamp}.example.com");
    let newer = format!("tie-newer-{stamp}.example.com");
    for domain in [&older, &newer] {
        let mut company = Company::new(domain);
        company.icp_fit_score = Some(88);
        db::companies::upsert(&pool, &company).await.unwrap();
    }
    let listed = db::companies::list_without_contacts(&pool, 100_000)
        .await
        .unwrap();
    let older_pos = listed.iter().position(|company| company.domain == older).unwrap();
    let newer_pos = listed.iter().position(|company| company.domain == newer).unwrap();
    assert!(older_pos < newer_pos);

    let capped = db::companies::list_without_contacts(&pool, 1).await.unwrap();
    assert_eq!(capped.len(), 1);
}
