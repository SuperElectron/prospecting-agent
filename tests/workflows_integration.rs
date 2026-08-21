use std::fmt::Write;

use prospecting_agent::config::MemoryConfig;
use prospecting_agent::db;
use prospecting_agent::domain::{Channel, EngagementKind, Signal, SignalKind, SignalStrength};
use prospecting_agent::memory::MemoryClient;
use prospecting_agent::workflows::sync::{RowSkip, SkipKind, ingest_person, ingest_signal, sync_dir};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so workflow tests cannot pass vacuously"
        );
        return None;
    };
    let pool = db::connect(&url).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    Some(pool)
}

macro_rules! require_pool {
    () => {
        match test_pool().await {
            Some(pool) => pool,
            None => {
                eprintln!("TEST_DATABASE_URL not set; skipping workflow integration test");
                return;
            }
        }
    };
}

fn memory_client(server: &MockServer) -> MemoryClient {
    MemoryClient::new(&MemoryConfig {
        base_url: server.uri(),
        user: "prospecting".into(),
    })
}

async fn mount_memorize_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m"})))
        .mount(server)
        .await;
}

fn write_dir(files: &[(&str, &str)]) -> String {
    let dir = std::env::temp_dir().join(format!("wf-sync-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        std::fs::write(dir.join(name), content).unwrap();
    }
    dir.display().to_string()
}

fn unique_domain(prefix: &str) -> String {
    format!("{prefix}-{}.example.com", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn csv_sync_lands_all_three_files_and_reruns_are_stable() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_memorize_ok(&server).await;
    let domain = unique_domain("full");
    let email = format!("vp-{}@{domain}", uuid::Uuid::new_v4().simple());
    let dir = write_dir(&[
        (
            "companies.csv",
            &format!(
                "domain,name,industry,employee_count,location\n{domain},Acme,Software,120,\"Austin, TX\"\n"
            ),
        ),
        (
            "contacts.csv",
            &format!(
                "email,first_name,last_name,title,company_domain,linkedin_url\n{email},Jane,Doe,VP Sales,{domain},\n"
            ),
        ),
        (
            "notes.csv",
            &format!("company_domain,note,noted_at\n{domain},Hiring reps,2026-08-01\n"),
        ),
    ]);
    let memory = memory_client(&server);
    let report = sync_dir(&pool, &memory, &dir).await.unwrap();
    assert_eq!(report.companies_upserted, 1);
    assert_eq!(report.contacts_upserted, 1);
    assert_eq!(report.notes_memorized, 1);
    assert!(report.skipped.is_empty());

    let again = sync_dir(&pool, &memory, &dir).await.unwrap();
    assert_eq!(again.contacts_upserted, 1);
    assert_eq!(again.notes_memorized, 0);
    assert_eq!(again.notes_already_imported, 1);
    let company = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(company.employee_count, Some(120));
    let contact = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    assert_eq!(contact.company_domain.as_deref(), Some(domain.as_str()));
}

#[tokio::test]
async fn malformed_rows_skip_with_reasons_and_good_rows_still_land() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_memorize_ok(&server).await;
    let good = unique_domain("good");
    let dir = write_dir(&[
        (
            "companies.csv",
            &format!(
                "domain,name,industry,employee_count,location\n{good},Good,Software,50,Austin\n,NoDomain,Software,10,Austin\nbad-count.example.com,Bad,Software,many,Austin\n"
            ),
        ),
        (
            "contacts.csv",
            "email,first_name,last_name,title,company_domain,linkedin_url\nnot-an-email,Jane,Doe,VP,,\n",
        ),
    ]);
    let memory = memory_client(&server);
    let report = sync_dir(&pool, &memory, &dir).await.unwrap();
    assert_eq!(report.companies_upserted, 1);
    assert_eq!(report.contacts_upserted, 0);
    assert_eq!(report.skipped.len(), 3);
    assert!(report.skipped.contains(&RowSkip {
        file: "companies.csv",
        line: 3,
        kind: SkipKind::Data,
        reason: "empty domain".into(),
    }));
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.file == "companies.csv" && s.line == 4 && s.reason.contains("not a number"))
    );
    assert!(report.skipped.iter().any(|s| s.file == "contacts.csv"
        && s.line == 2
        && s.kind == SkipKind::Data
        && s.reason.contains("implausible email")));
    assert!(db::companies::by_domain(&pool, &good).await.unwrap().is_some());
}

#[tokio::test]
async fn memory_backend_failure_skips_notes_but_keeps_the_batch_alive() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"error": "Memory client is not available"})),
        )
        .mount(&server)
        .await;
    let domain = unique_domain("membad");
    let dir = write_dir(&[(
        "notes.csv",
        &format!(
            "company_domain,note,noted_at\n{domain},First note,2026-08-01\n{domain},Second note,2026-08-02\n"
        ),
    )]);
    let report = sync_dir(&pool, &memory_client(&server), &dir).await.unwrap();
    assert_eq!(report.notes_memorized, 0);
    assert_eq!(report.skipped.len(), 2);
    assert!(report.skipped.iter().all(|s| s.kind == SkipKind::Backend));
    assert!(report.fatal.is_none());
}

#[tokio::test]
async fn repeated_backend_failures_stop_the_note_import_with_a_fatal_report() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"error": "Memory client is not available"})),
        )
        .mount(&server)
        .await;
    let domain = unique_domain("fatal");
    let mut rows = String::from("company_domain,note,noted_at\n");
    for i in 0..5 {
        let _ = writeln!(rows, "{domain},Note number {i},2026-08-0{}", i + 1);
    }
    let dir = write_dir(&[("notes.csv", &rows)]);
    let report = sync_dir(&pool, &memory_client(&server), &dir).await.unwrap();
    assert_eq!(report.skipped.len(), 3);
    assert!(report.fatal.is_some());
}

#[tokio::test]
async fn reimport_preserves_enrichment_and_pipeline_state() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_memorize_ok(&server).await;
    let domain = unique_domain("preserve");
    let email = format!("keep-{}@{domain}", uuid::Uuid::new_v4().simple());
    let dir = write_dir(&[
        (
            "companies.csv",
            &format!(
                "domain,name,industry,employee_count,location
{domain},Keeper,,50,
"
            ),
        ),
        (
            "contacts.csv",
            &format!(
                "email,first_name,last_name,title,company_domain,linkedin_url
{email},Kai,,,,
"
            ),
        ),
    ]);
    let memory = memory_client(&server);
    sync_dir(&pool, &memory, &dir).await.unwrap();

    let mut company = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    company.summary = Some("hand-written research summary".into());
    company.icp_fit_score = Some(88);
    db::companies::upsert(&pool, &company).await.unwrap();
    let mut contact = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    contact.status = prospecting_agent::domain::ContactStatus::InSequence;
    contact.score = Some(77);
    contact.assigned_sender = Some("sales@ours.io".into());
    db::contacts::upsert(&pool, &contact).await.unwrap();

    sync_dir(&pool, &memory, &dir).await.unwrap();
    let company_after = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(
        company_after.summary.as_deref(),
        Some("hand-written research summary")
    );
    assert_eq!(company_after.icp_fit_score, Some(88));
    assert_eq!(company_after.employee_count, Some(50));
    let contact_after = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    assert_eq!(
        contact_after.status,
        prospecting_agent::domain::ContactStatus::InSequence
    );
    assert_eq!(contact_after.score, Some(77));
    assert_eq!(contact_after.assigned_sender.as_deref(), Some("sales@ours.io"));
    assert_eq!(contact_after.first_name.as_deref(), Some("Kai"));
}

#[tokio::test]
async fn missing_files_are_fine_and_empty_dir_reports_zero() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    let dir = write_dir(&[]);
    let report = sync_dir(&pool, &memory_client(&server), &dir).await.unwrap();
    assert_eq!(report.companies_upserted, 0);
    assert_eq!(report.contacts_upserted, 0);
    assert_eq!(report.notes_memorized, 0);
}

#[tokio::test]
async fn apollo_person_ingest_lands_contact_company_and_memory_line() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .and(body_partial_json(serde_json::json!({"infer": false})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m"})))
        .expect(1)
        .mount(&server)
        .await;
    let domain = unique_domain("apollo");
    let email = format!("cro-{}@{domain}", uuid::Uuid::new_v4().simple());
    let apollo_id = format!("p-{}", uuid::Uuid::new_v4().simple());
    let person: prospecting_agent::clients::ApolloPerson = serde_json::from_value(serde_json::json!({
        "id": apollo_id,
        "first_name": "Casey",
        "title": "CRO",
        "email": email,
        "seniority": "c_suite",
        "organization": {"id": "o-9", "name": "Apollo Co", "primary_domain": domain,
                          "estimated_num_employees": 80},
    }))
    .unwrap();
    let contact_id = ingest_person(&pool, &memory_client(&server), &person)
        .await
        .unwrap();
    let contact = db::contacts::by_id(&pool, contact_id).await.unwrap().unwrap();
    assert_eq!(contact.email.as_deref(), Some(email.as_str()));
    let company = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(company.employee_count, Some(80));
}

#[tokio::test]
async fn signal_ingest_is_idempotent_and_memorizes_a_tagged_line() {
    let pool = require_pool!();
    let server = MockServer::start().await;
    mount_memorize_ok(&server).await;
    let domain = unique_domain("sig");
    let signal = Signal::new(&domain, SignalKind::Funding, SignalStrength::Strong, "raised B");
    let memory = memory_client(&server);
    ingest_signal(&pool, &memory, &signal).await.unwrap();
    ingest_signal(&pool, &memory, &signal).await.unwrap();
    let found = db::signals::for_domain(&pool, &domain, 10).await.unwrap();
    assert_eq!(found.len(), 1);
}

fn apollo_client(server: &MockServer) -> prospecting_agent::clients::ApolloClient {
    prospecting_agent::clients::ApolloClient::with_base_url(
        prospecting_agent::config::Secret::new("apollo-test"),
        &server.uri(),
    )
}

fn search_person_json(id: &str, domain: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "first_name": "Obfuscated", "title": "VP Sales",
        "organization": {"id": "o-1", "primary_domain": domain}})
}

fn matched_person_json(id: &str, email: &str, domain: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "first_name": "Jane", "title": "VP Sales", "email": email,
        "seniority": "vp", "organization": {"id": "o-1", "primary_domain": domain,
        "estimated_num_employees": 90}})
}

#[tokio::test]
async fn discovery_matches_new_people_and_skips_known_crm_ids() {
    let pool = require_pool!();
    let apollo_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("disc");
    let known_id = format!("known-{}", uuid::Uuid::new_v4().simple());
    let fresh_id = format!("fresh-{}", uuid::Uuid::new_v4().simple());
    let email = format!("vp-{}@{domain}", uuid::Uuid::new_v4().simple());

    let mut known = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Apollo);
    known.crm_id = Some(known_id.clone());
    db::contacts::upsert(&pool, &known).await.unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/mixed_people/api_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "people": [search_person_json(&known_id, &domain), search_person_json(&fresh_id, &domain)],
            "total_entries": 2,
        })))
        .expect(1)
        .mount(&apollo_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .and(body_partial_json(serde_json::json!({"id": fresh_id})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"person": matched_person_json(&fresh_id, &email, &domain)}),
            ),
        )
        .expect(1)
        .mount(&apollo_server)
        .await;

    let icp = prospecting_agent::config::IcpCriteria::default();
    let budget = prospecting_agent::workflows::discovery::DiscoveryBudget::default();
    let mut credits = budget.max_credits_per_run;
    let report = prospecting_agent::workflows::discovery::discover_contacts(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        &icp,
        &domain,
        &budget,
        &mut credits,
    )
    .await
    .unwrap();
    assert_eq!(report.discovered, 1);
    assert_eq!(report.already_known, 1);
    assert_eq!(report.credits_spent, 1);
    let landed = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    assert_eq!(landed.status, prospecting_agent::domain::ContactStatus::Enriched);
    assert_eq!(landed.crm_id.as_deref(), Some(fresh_id.as_str()));
}

#[tokio::test]
async fn discovery_stops_when_the_credit_budget_runs_out() {
    let pool = require_pool!();
    let apollo_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("budget");
    let people: Vec<serde_json::Value> = (0..5)
        .map(|i| search_person_json(&format!("cand-{}-{i}", uuid::Uuid::new_v4().simple()), &domain))
        .collect();
    Mock::given(method("POST"))
        .and(path("/v1/mixed_people/api_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "people": people, "total_entries": 5,
        })))
        .mount(&apollo_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"person": null})))
        .expect(2)
        .mount(&apollo_server)
        .await;
    let icp = prospecting_agent::config::IcpCriteria::default();
    let budget = prospecting_agent::workflows::discovery::DiscoveryBudget {
        contacts_per_account: 5,
        max_credits_per_run: 2,
    };
    let mut credits = budget.max_credits_per_run;
    let report = prospecting_agent::workflows::discovery::discover_contacts(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        &icp,
        &domain,
        &budget,
        &mut credits,
    )
    .await
    .unwrap();
    assert_eq!(report.credits_spent, 2);
    assert_eq!(credits, 0);
    assert_eq!(report.no_match, 2);
}

#[tokio::test]
async fn contact_enrichment_marks_new_contacts_enriched() {
    let pool = require_pool!();
    let apollo_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("enr");
    let email = format!("new-{}@{domain}", uuid::Uuid::new_v4().simple());
    let mut contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    contact.email = Some(email.clone());
    db::contacts::upsert(&pool, &contact).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .and(body_partial_json(serde_json::json!({"email": email})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"person": matched_person_json(&format!("enr-{}", uuid::Uuid::new_v4().simple()), &email, &domain)})),
        )
        .mount(&apollo_server)
        .await;
    let mut credits: u16 = 500;
    let report = prospecting_agent::workflows::discovery::enrich_contacts(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        500,
        &mut credits,
    )
    .await
    .unwrap();
    assert!(report.enriched >= 1);
    assert!(report.credits_spent >= 1);
    let after = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    assert_eq!(after.status, prospecting_agent::domain::ContactStatus::Enriched);
    assert_eq!(after.seniority, Some(prospecting_agent::domain::Seniority::Vp));
}

#[tokio::test]
async fn matched_email_of_known_contact_persists_the_crm_id_association() {
    let pool = require_pool!();
    let apollo_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("assoc");
    let email = format!("known-{}@{domain}", uuid::Uuid::new_v4().simple());
    let candidate_id = format!("cand-{}", uuid::Uuid::new_v4().simple());
    let mut existing = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    existing.email = Some(email.clone());
    db::contacts::upsert(&pool, &existing).await.unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/mixed_people/api_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "people": [search_person_json(&candidate_id, &domain)], "total_entries": 1,
        })))
        .mount(&apollo_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"person": matched_person_json(&candidate_id, &email, &domain)}),
        ))
        .expect(1)
        .mount(&apollo_server)
        .await;
    let icp = prospecting_agent::config::IcpCriteria::default();
    let budget = prospecting_agent::workflows::discovery::DiscoveryBudget::default();
    let mut credits = budget.max_credits_per_run;
    let first = prospecting_agent::workflows::discovery::discover_contacts(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        &icp,
        &domain,
        &budget,
        &mut credits,
    )
    .await
    .unwrap();
    assert_eq!(first.already_known, 1);
    assert_eq!(first.credits_spent, 1);
    let after = db::contacts::by_email(&pool, &email).await.unwrap().unwrap();
    assert_eq!(after.crm_id.as_deref(), Some(candidate_id.as_str()));

    let second = prospecting_agent::workflows::discovery::discover_contacts(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        &icp,
        &domain,
        &budget,
        &mut credits,
    )
    .await
    .unwrap();
    assert_eq!(second.credits_spent, 0);
    assert_eq!(second.already_known, 1);
}

#[tokio::test]
async fn locked_placeholder_email_counts_as_no_email_and_lands_nothing() {
    let pool = require_pool!();
    let apollo_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("locked");
    let candidate_id = format!("lock-{}", uuid::Uuid::new_v4().simple());
    Mock::given(method("POST"))
        .and(path("/v1/mixed_people/api_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "people": [search_person_json(&candidate_id, &domain)], "total_entries": 1,
        })))
        .mount(&apollo_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "person": {"id": candidate_id, "first_name": "Locked",
                        "email": "email_not_unlocked@domain.com",
                        "organization": {"id": "o", "primary_domain": domain}},
        })))
        .mount(&apollo_server)
        .await;
    let icp = prospecting_agent::config::IcpCriteria::default();
    let budget = prospecting_agent::workflows::discovery::DiscoveryBudget::default();
    let mut credits = budget.max_credits_per_run;
    let report = prospecting_agent::workflows::discovery::discover_contacts(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        &icp,
        &domain,
        &budget,
        &mut credits,
    )
    .await
    .unwrap();
    assert_eq!(report.no_email, 1);
    assert_eq!(report.discovered, 0);
    assert!(
        db::contacts::by_crm_id(&pool, &candidate_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn company_enrichment_preserves_scoring_state() {
    let pool = require_pool!();
    let apollo_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("keepscore");
    let mut company = prospecting_agent::domain::Company::new(&domain);
    company.icp_fit_score = Some(91);
    company.summary = None;
    db::companies::upsert(&pool, &company).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/organizations/enrich"))
        .and(body_partial_json(serde_json::json!({"domain": domain})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "organization": {"id": "o-1", "primary_domain": domain, "industry": "Software",
                              "estimated_num_employees": 75, "short_description": "Makes tools"},
        })))
        .expect(1)
        .mount(&apollo_server)
        .await;
    let mut credits: u16 = 5000;
    let report = prospecting_agent::workflows::discovery::enrich_companies(
        &pool,
        &memory_client(&memory_server),
        &apollo_client(&apollo_server),
        10_000,
        &mut credits,
    )
    .await
    .unwrap();
    assert!(report.enriched >= 1);
    let after = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(after.icp_fit_score, Some(91));
    assert_eq!(after.industry.as_deref(), Some("Software"));
    assert_eq!(after.summary.as_deref(), Some("Makes tools"));

    let listed = db::companies::list_unenriched(&pool, 500).await.unwrap();
    assert!(listed.iter().all(|c| c.domain != domain));
}

fn tavily_client(server: &MockServer) -> prospecting_agent::clients::TavilyClient {
    prospecting_agent::clients::TavilyClient::with_base_url(
        prospecting_agent::config::Secret::new("tvly-test"),
        &server.uri(),
    )
}

fn llm_client(server: &MockServer) -> prospecting_agent::llm::LlmClient {
    prospecting_agent::llm::LlmClient::new(&prospecting_agent::config::LlmConfig {
        base_url: format!("{}/v1", server.uri()),
        api_key: prospecting_agent::config::Secret::new("test"),
        model: "gpt-oss-120b".into(),
    })
}

fn completion_with(content: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"choices": [{"message": {"role": "assistant",
        "content": content.to_string()}}]})
}

#[tokio::test]
async fn company_research_updates_summary_and_memorizes_angles() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("research");
    let company = prospecting_agent::domain::Company::new(&domain);
    db::companies::upsert(&pool, &company).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "answer": "They ship developer tools.",
            "results": [{"title": "About", "url": "https://x.example.com", "content": "Dev tools",
                         "score": 0.9, "published_date": null}],
        })))
        .mount(&tavily_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "summary": "Developer tooling startup in Austin.",
                "buying_signals": ["hiring"],
                "pain_points": ["manual outreach"],
                "personalization_angles": ["their new release"],
            }))),
        )
        .mount(&llm_server)
        .await;
    let outcome = prospecting_agent::workflows::research::research_company(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &prospecting_agent::llm::default_policies(),
        &domain,
    )
    .await
    .unwrap();
    assert_eq!(outcome.research.summary, "Developer tooling startup in Austin.");
    assert_eq!(outcome.sources, vec!["https://x.example.com"]);
    let after = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(
        after.summary.as_deref(),
        Some("Developer tooling startup in Austin.")
    );
}

#[tokio::test]
async fn research_preserves_enrichment_fields_and_bumps_updated_at() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("keepres");
    let mut company = prospecting_agent::domain::Company::new(&domain);
    company.industry = Some("Software".into());
    company.employee_count = Some(64);
    company.icp_fit_score = Some(83);
    db::companies::upsert(&pool, &company).await.unwrap();
    let before = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "answer": null,
            "results": [{"title": "t", "url": "https://r.example.com", "content": "c",
                         "score": 0.5, "published_date": null}],
        })))
        .mount(&tavily_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "summary": "fresh summary",
                "buying_signals": [],
                "pain_points": [],
                "personalization_angles": [],
            }))),
        )
        .mount(&llm_server)
        .await;
    prospecting_agent::workflows::research::research_company(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &[],
        &domain,
    )
    .await
    .unwrap();
    let after = db::companies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(after.industry.as_deref(), Some("Software"));
    assert_eq!(after.employee_count, Some(64));
    assert_eq!(after.icp_fit_score, Some(83));
    assert_eq!(after.summary.as_deref(), Some("fresh summary"));
    assert!(after.updated_at > before.updated_at);
}

#[tokio::test]
async fn empty_news_results_skip_the_llm_entirely() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    let domain = unique_domain("nonews");
    db::companies::upsert(&pool, &prospecting_agent::domain::Company::new(&domain))
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"answer": null, "results": []})),
        )
        .mount(&tavily_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&llm_server)
        .await;
    let report = prospecting_agent::workflows::research::detect_signals(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &[],
        &domain,
    )
    .await
    .unwrap();
    assert_eq!(report.detected, 0);
}

#[tokio::test]
async fn memory_outage_does_not_fail_company_research() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"error": "Memory client is not available"})),
        )
        .mount(&memory_server)
        .await;
    let domain = unique_domain("memout");
    db::companies::upsert(&pool, &prospecting_agent::domain::Company::new(&domain))
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "answer": null,
            "results": [{"title": "t", "url": "https://r.example.com", "content": "c",
                         "score": 0.5, "published_date": null}],
        })))
        .mount(&tavily_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "summary": "resilient summary",
                "buying_signals": [],
                "pain_points": [],
                "personalization_angles": ["angle one"],
            }))),
        )
        .mount(&llm_server)
        .await;
    let outcome = prospecting_agent::workflows::research::research_company(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &[],
        &domain,
    )
    .await
    .unwrap();
    assert_eq!(outcome.research.summary, "resilient summary");
}

#[tokio::test]
async fn signal_detection_on_an_unknown_company_is_a_typed_error() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    let err = prospecting_agent::workflows::research::detect_signals(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &[],
        &unique_domain("nosuch"),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        prospecting_agent::workflows::research::ResearchError::UnknownCompany(_)
    ));
}

#[tokio::test]
async fn research_on_an_unknown_company_is_a_typed_error() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    let err = prospecting_agent::workflows::research::research_company(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &[],
        &unique_domain("ghost"),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        prospecting_agent::workflows::research::ResearchError::UnknownCompany(_)
    ));
}

#[tokio::test]
async fn signal_detection_maps_kinds_and_counts_unknowns() {
    let pool = require_pool!();
    let tavily_server = MockServer::start().await;
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    let domain = unique_domain("sigdet");
    db::companies::upsert(&pool, &prospecting_agent::domain::Company::new(&domain))
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(serde_json::json!({"topic": "news"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "answer": null,
            "results": [{"title": "Raise", "url": "https://n.example.com", "content": "Raised $20M",
                         "score": 0.95, "published_date": "2026-08-10"}],
        })))
        .mount(&tavily_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "signals": [
                    {"kind": "funding", "strength": "strong", "summary": "Raised a $20M round",
                     "source_url": "https://n.example.com"},
                    {"kind": "meteor strike", "strength": "strong", "summary": "irrelevant",
                     "source_url": null},
                ],
            }))),
        )
        .mount(&llm_server)
        .await;
    let report = prospecting_agent::workflows::research::detect_signals(
        &pool,
        &memory_client(&memory_server),
        &tavily_client(&tavily_server),
        &llm_client(&llm_server),
        &[],
        &domain,
    )
    .await
    .unwrap();
    assert_eq!(report.detected, 2);
    assert_eq!(report.ingested, 1);
    assert_eq!(report.unknown_kind, 1);
    let signals = db::signals::for_domain(&pool, &domain, 10).await.unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, SignalKind::Funding);
    assert_eq!(signals[0].source_url.as_deref(), Some("https://n.example.com"));
}

#[tokio::test]
#[ignore = "live research smoke: needs TEST_RESEARCH_LIVE, DGX LLM, tavily key, postgres, memory, and the jasper.ai seed row (run sync-csv first)"]
async fn live_company_research_end_to_end() {
    if std::env::var("TEST_RESEARCH_LIVE").is_err() {
        eprintln!("TEST_RESEARCH_LIVE not set; skipping live research smoke");
        return;
    }
    let pool = require_pool!();
    let config = prospecting_agent::config::AppConfig::from_env().expect("full env");
    let memory = MemoryClient::new(&config.memory);
    let tavily = prospecting_agent::clients::TavilyClient::new(config.tavily_api_key.clone());
    let llm = prospecting_agent::llm::LlmClient::new(&config.llm);
    let outcome = prospecting_agent::workflows::research::research_company(
        &pool,
        &memory,
        &tavily,
        &llm,
        &prospecting_agent::llm::default_policies(),
        "jasper.ai",
    )
    .await
    .unwrap();
    assert!(!outcome.research.summary.is_empty());
    eprintln!("live research summary: {}", outcome.research.summary);
    let report = prospecting_agent::workflows::research::detect_signals(
        &pool,
        &memory,
        &tavily,
        &llm,
        &prospecting_agent::llm::default_policies(),
        "jasper.ai",
    )
    .await
    .unwrap();
    eprintln!(
        "live signals: detected {} ingested {} unknown {}",
        report.detected, report.ingested, report.unknown_kind
    );
    assert!(report.detected >= report.ingested);
}

async fn mount_recall_empty(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/filter"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [], "total": 0, "page": 1, "size": 50, "pages": 0,
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn account_strategy_persists_assessment_from_db_context() {
    let pool = require_pool!();
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_memorize_ok(&memory_server).await;
    mount_recall_empty(&memory_server).await;
    let domain = unique_domain("strat");
    let mut company = prospecting_agent::domain::Company::new(&domain);
    company.summary = Some("Austin devtools company".into());
    db::companies::upsert(&pool, &company).await.unwrap();
    let mut contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    contact.email = Some(format!("vp-{}@{domain}", uuid::Uuid::new_v4().simple()));
    contact.company_domain = Some(domain.clone());
    db::contacts::upsert(&pool, &contact).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "stage": "engaged",
                "health": "watch",
                "coordination_flags": ["carpet_bomb_risk", "made_up_flag"],
                "summary": "Two active threads; coordinate before adding more.",
            }))),
        )
        .expect(1)
        .mount(&llm_server)
        .await;
    let strategy = prospecting_agent::workflows::accounts::evaluate_account_strategy(
        &pool,
        &memory_client(&memory_server),
        &llm_client(&llm_server),
        &prospecting_agent::llm::default_policies(),
        &domain,
    )
    .await
    .unwrap();
    assert_eq!(strategy.stage, prospecting_agent::domain::AccountStage::Engaged);
    let stored = db::strategies::by_domain(&pool, &domain).await.unwrap().unwrap();
    assert_eq!(stored.health, prospecting_agent::domain::AccountHealth::Watch);
    assert_eq!(stored.coordination_flags, vec!["carpet_bomb_risk"]);
}

#[tokio::test]
async fn preflight_delays_on_carpet_bomb_when_recent_sends_hit_the_cap() {
    let pool = require_pool!();
    let domain = unique_domain("carpet");
    let strategy = prospecting_agent::domain::AccountStrategy {
        domain: domain.clone(),
        stage: prospecting_agent::domain::AccountStage::Engaged,
        health: prospecting_agent::domain::AccountHealth::Healthy,
        coordination_flags: vec![prospecting_agent::domain::FLAG_CARPET_BOMB.into()],
        summary: "busy account".into(),
        updated_at: chrono::Utc::now(),
    };
    db::strategies::upsert(&pool, &strategy).await.unwrap();
    for step in 0..2u8 {
        let mut touched =
            prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
        touched.email = Some(format!("t{step}-{}@{domain}", uuid::Uuid::new_v4().simple()));
        touched.company_domain = Some(domain.clone());
        db::contacts::upsert(&pool, &touched).await.unwrap();
        let mut sent =
            prospecting_agent::domain::Engagement::outbound(touched.id, Channel::Email, EngagementKind::Sent);
        sent.sequence_step = Some(step);
        db::engagements::insert(&pool, &sent).await.unwrap();
    }
    let mut fresh = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    fresh.email = Some(format!("fresh-{}@{domain}", uuid::Uuid::new_v4().simple()));
    fresh.company_domain = Some(domain.clone());
    db::contacts::upsert(&pool, &fresh).await.unwrap();
    let config = prospecting_agent::workflows::accounts::PreflightConfig::default();
    let decision = prospecting_agent::workflows::accounts::preflight(&pool, &config, &fresh)
        .await
        .unwrap();
    assert!(matches!(
        decision,
        prospecting_agent::workflows::accounts::PreflightDecision::Delay { .. }
    ));

    let touched_contact = db::contacts::list_by_company_domain(&pool, &domain)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id != fresh.id)
        .unwrap();
    let in_flight = prospecting_agent::workflows::accounts::preflight(&pool, &config, &touched_contact)
        .await
        .unwrap();
    assert_eq!(
        in_flight,
        prospecting_agent::workflows::accounts::PreflightDecision::Proceed
    );

    sqlx::query("UPDATE engagements SET occurred_at = now() - interval '30 days' WHERE contact_id IN (SELECT id FROM contacts WHERE company_domain = $1)")
        .bind(&domain)
        .execute(&pool)
        .await
        .unwrap();
    let aged_out = prospecting_agent::workflows::accounts::preflight(&pool, &config, &fresh)
        .await
        .unwrap();
    assert_eq!(
        aged_out,
        prospecting_agent::workflows::accounts::PreflightDecision::Proceed
    );
}

#[tokio::test]
async fn preflight_blocks_customers_and_modifies_new_contacts_at_advanced_accounts() {
    let pool = require_pool!();
    let domain = unique_domain("gate");
    let mut strategy = prospecting_agent::domain::AccountStrategy {
        domain: domain.clone(),
        stage: prospecting_agent::domain::AccountStage::Customer,
        health: prospecting_agent::domain::AccountHealth::Healthy,
        coordination_flags: vec![],
        summary: "they bought".into(),
        updated_at: chrono::Utc::now(),
    };
    db::strategies::upsert(&pool, &strategy).await.unwrap();
    let mut contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    contact.company_domain = Some(domain.clone());
    let config = prospecting_agent::workflows::accounts::PreflightConfig::default();
    let blocked = prospecting_agent::workflows::accounts::preflight(&pool, &config, &contact)
        .await
        .unwrap();
    assert!(matches!(
        blocked,
        prospecting_agent::workflows::accounts::PreflightDecision::Block { .. }
    ));

    strategy.stage = prospecting_agent::domain::AccountStage::Engaged;
    strategy.coordination_flags = vec![prospecting_agent::domain::FLAG_NEW_CONTACT_ADVANCED.into()];
    db::strategies::upsert(&pool, &strategy).await.unwrap();
    contact.status = prospecting_agent::domain::ContactStatus::Enriched;
    let modified = prospecting_agent::workflows::accounts::preflight(&pool, &config, &contact)
        .await
        .unwrap();
    assert!(matches!(
        modified,
        prospecting_agent::workflows::accounts::PreflightDecision::Modify { .. }
    ));

    contact.status = prospecting_agent::domain::ContactStatus::InSequence;
    let proceed = prospecting_agent::workflows::accounts::preflight(&pool, &config, &contact)
        .await
        .unwrap();
    assert_eq!(
        proceed,
        prospecting_agent::workflows::accounts::PreflightDecision::Proceed
    );
}

#[tokio::test]
async fn email_generation_grounds_in_context_and_validates_rules() {
    let pool = require_pool!();
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_recall_empty(&memory_server).await;
    let domain = unique_domain("gen");
    let mut company = prospecting_agent::domain::Company::new(&domain);
    company.summary = Some("They build billing APIs for SaaS teams.".into());
    db::companies::upsert(&pool, &company).await.unwrap();
    let mut contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    contact.email = Some(format!("vp-{}@{domain}", uuid::Uuid::new_v4().simple()));
    contact.first_name = Some("Jane".into());
    contact.company_domain = Some(domain.clone());
    db::contacts::upsert(&pool, &contact).await.unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
            "subject": "billing question",
            "body": "Hi Jane,\n\nSaw the billing API work. Worth a quick chat about rollout speed?\n\nBest",
            "personalization_fact": "they build billing APIs",
        }))))
        .expect(1)
        .mount(&llm_server)
        .await;
    let email = prospecting_agent::workflows::outreach::generate_email(
        &pool,
        &memory_client(&memory_server),
        &llm_client(&llm_server),
        &prospecting_agent::llm::default_policies(),
        &prospecting_agent::config::MessagingRules::default(),
        &contact,
        prospecting_agent::workflows::outreach::EmailContext {
            step: 1,
            max_steps: 3,
            is_first_touch: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(email.subject, "billing question");
    assert!(email.body_html.contains("<p>Hi Jane,</p>"));
    assert_eq!(email.step, 1);
}

#[tokio::test]
async fn rule_violations_trigger_one_retry_then_typed_failure() {
    let pool = require_pool!();
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    mount_recall_empty(&memory_server).await;
    let contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    let mut with_email = contact.clone();
    with_email.email = Some("target@example.com".into());
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "subject": "s",
                "body": "Let's leverage synergy across the board.",
                "personalization_fact": "f",
            }))),
        )
        .expect(2)
        .mount(&llm_server)
        .await;
    let err = prospecting_agent::workflows::outreach::generate_email(
        &pool,
        &memory_client(&memory_server),
        &llm_client(&llm_server),
        &[],
        &prospecting_agent::config::MessagingRules::default(),
        &with_email,
        prospecting_agent::workflows::outreach::EmailContext {
            step: 1,
            max_steps: 3,
            is_first_touch: true,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        prospecting_agent::workflows::outreach::OutreachError::RulesViolated(_)
    ));
}

#[tokio::test]
async fn generation_without_an_email_address_is_a_typed_error() {
    let pool = require_pool!();
    let llm_server = MockServer::start().await;
    let memory_server = MockServer::start().await;
    let contact = prospecting_agent::domain::Contact::new(prospecting_agent::domain::ContactSource::Csv);
    let err = prospecting_agent::workflows::outreach::generate_email(
        &pool,
        &memory_client(&memory_server),
        &llm_client(&llm_server),
        &[],
        &prospecting_agent::config::MessagingRules::default(),
        &contact,
        prospecting_agent::workflows::outreach::EmailContext {
            step: 1,
            max_steps: 3,
            is_first_touch: true,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        prospecting_agent::workflows::outreach::OutreachError::NoEmail(_)
    ));
}

#[tokio::test]
#[ignore = "live generation smoke: needs TEST_GENERATION_LIVE, DGX LLM, postgres, memory, and jasper.ai seed + research rows"]
async fn live_email_generation_end_to_end() {
    if std::env::var("TEST_GENERATION_LIVE").is_err() {
        eprintln!("TEST_GENERATION_LIVE not set; skipping live generation smoke");
        return;
    }
    let pool = require_pool!();
    let config = prospecting_agent::config::AppConfig::from_env().expect("full env");
    let memory = MemoryClient::new(&config.memory);
    let llm = prospecting_agent::llm::LlmClient::new(&config.llm);
    let contact = db::contacts::list_by_company_domain(&pool, "jasper.ai")
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.email.is_some())
        .expect("jasper.ai contact from sync-csv");
    let email = prospecting_agent::workflows::outreach::generate_email(
        &pool,
        &memory,
        &llm,
        &prospecting_agent::llm::default_policies(),
        &prospecting_agent::config::MessagingRules::default(),
        &contact,
        prospecting_agent::workflows::outreach::EmailContext {
            step: 1,
            max_steps: 3,
            is_first_touch: true,
        },
    )
    .await
    .unwrap();
    eprintln!("live subject: {}", email.subject);
    eprintln!("live body:\n{}", email.body_text);
    eprintln!("live fact: {}", email.personalization_fact);
    assert!(!email.subject.is_empty());
    assert!(!email.body_text.is_empty());
}
