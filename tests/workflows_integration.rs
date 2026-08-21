use prospecting_agent::config::MemoryConfig;
use prospecting_agent::db;
use prospecting_agent::domain::{Signal, SignalKind, SignalStrength};
use prospecting_agent::memory::MemoryClient;
use prospecting_agent::workflows::sync::{ingest_person, ingest_signal, sync_dir};
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
    assert!(report.skipped.iter().any(|s| s.reason.contains("empty domain")));
    assert!(report.skipped.iter().any(|s| s.reason.contains("not a number")));
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason.contains("implausible email"))
    );
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
    let person: prospecting_agent::clients::ApolloPerson = serde_json::from_value(serde_json::json!({
        "id": "p-9",
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
