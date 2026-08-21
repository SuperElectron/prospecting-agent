use chrono::{TimeZone, Utc};
use prospecting_agent::config::{AppConfig, MessagingRules, Secret};
use prospecting_agent::connectors::gmail::{GmailConnector, OauthClient, SenderAccount};
use prospecting_agent::db;
use prospecting_agent::domain::ContactStatus;
use prospecting_agent::jobs::{JobContext, JobKind, run_job};
use prospecting_agent::workflows::outreach::{SendPassInputs, run_send_pass};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Rig {
    ctx: JobContext,
    apollo: MockServer,
    tavily: MockServer,
    llm: MockServer,
    memory: MockServer,
    gmail: MockServer,
    domain: String,
    email: String,
}

fn completion_with(content: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"choices": [{"message": {"role": "assistant",
        "content": content.to_string()}}]})
}

async fn rig() -> Option<Rig> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CI").is_err(),
            "TEST_DATABASE_URL must be set in CI so the e2e suite cannot pass vacuously"
        );
        eprintln!("TEST_DATABASE_URL not set; skipping e2e pipeline test");
        return None;
    };
    let pool = db::connect(&url).await.expect("connect to test database");
    db::migrate(&pool).await.expect("run migrations");

    let stamp = uuid::Uuid::new_v4().simple().to_string();
    let domain = format!("e2e-{stamp}.example.com");
    let email = format!("vp-{stamp}@{domain}");
    let dir = std::env::temp_dir().join(format!("e2e-{stamp}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("companies.csv"),
        format!(
            "domain,name,industry,employee_count,location\n{domain},E2E Co,Software,120,\"Austin, TX\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("contacts.csv"),
        format!("email,first_name,last_name,title,company_domain,linkedin_url\n{email},Casey,Lee,VP Sales,{domain},\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("notes.csv"),
        format!("company_domain,note,noted_at\n{domain},Hiring several AEs this quarter,2026-08-01\n"),
    )
    .unwrap();

    let apollo = MockServer::start().await;
    let tavily = MockServer::start().await;
    let llm = MockServer::start().await;
    let memory = MockServer::start().await;
    let gmail = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/memories/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "m"})))
        .mount(&memory)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/memories/filter"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [], "total": 0, "page": 1, "size": 50, "pages": 0,
        })))
        .mount(&memory)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token": "at-1", "expires_in": 3599})),
        )
        .mount(&gmail)
        .await;

    let env: prospecting_agent::config::EnvMap = [
        ("DATABASE_URL", url.as_str()),
        ("LLM_BASE_URL", "http://127.0.0.1:1/v1"),
        ("LLM_MODEL", "test-model"),
        ("MEMORY_URL", "http://127.0.0.1:1"),
        ("APOLLO_API_KEY", "test-apollo"),
        ("TAVILY_API_KEY", "test-tavily"),
        ("GMAIL_CLIENT_FILE", "/nonexistent/client.json"),
        ("GMAIL_SENDERS_FILE", "/nonexistent/senders.json"),
        ("DRY_RUN", "false"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .chain(std::iter::once((
        "CSV_DATA_DIR".to_string(),
        dir.display().to_string(),
    )))
    .collect();
    let config = AppConfig::from_map(&env).expect("test config parses");
    let mut ctx = JobContext::from_config(pool.clone(), config);
    ctx.apollo =
        prospecting_agent::clients::ApolloClient::with_base_url(Secret::new("test-apollo"), &apollo.uri());
    ctx.tavily =
        prospecting_agent::clients::TavilyClient::with_base_url(Secret::new("test-tavily"), &tavily.uri());
    ctx.llm = prospecting_agent::llm::LlmClient::new(&prospecting_agent::config::LlmConfig {
        base_url: format!("{}/v1", llm.uri()),
        api_key: Secret::new("test"),
        model: "test-model".into(),
    });
    ctx.memory = prospecting_agent::memory::MemoryClient::new(&prospecting_agent::config::MemoryConfig {
        base_url: memory.uri(),
        user: "test".into(),
    });
    let oauth = OauthClient::new("cid", Secret::new("csec")).with_bases(&gmail.uri(), &gmail.uri());
    let sender = SenderAccount {
        email: "sender@e2e.example.com".into(),
        name: "E2E Sender".into(),
        refresh_token: "rt-e2e".into(),
        daily_limit: 50,
    };
    ctx.gmail = Some(GmailConnector::new(oauth, vec![sender], pool).with_api_base(&gmail.uri()));

    Some(Rig {
        ctx,
        apollo,
        tavily,
        llm,
        memory,
        gmail,
        domain,
        email,
    })
}

#[tokio::test]
async fn full_funnel_from_csv_to_reply_and_report() {
    let Some(rig) = rig().await else { return };
    let Rig {
        ctx,
        apollo,
        tavily,
        llm,
        memory: _memory,
        gmail,
        domain,
        email,
    } = rig;

    let sync = run_job(&ctx, JobKind::CsvSync).await.unwrap();
    assert_eq!(sync["companies_upserted"], 1);
    assert_eq!(sync["contacts_upserted"], 1);
    let contact = db::contacts::by_email(&ctx.pool, &email).await.unwrap().unwrap();
    assert_eq!(contact.status, ContactStatus::New);

    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .and(body_string_contains(&email))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "person": {"id": format!("e2e-{email}"), "first_name": "Casey", "title": "VP Sales",
                        "email": email, "seniority": "vp",
                        "organization": {"id": "o-1", "primary_domain": domain,
                                          "estimated_num_employees": 120}},
        })))
        .mount(&apollo)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/people/match"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"person": null})))
        .mount(&apollo)
        .await;
    run_job(&ctx, JobKind::EnrichContacts).await.unwrap();
    let contact = db::contacts::by_email(&ctx.pool, &email).await.unwrap().unwrap();
    assert_eq!(contact.status, ContactStatus::Enriched);

    run_job(&ctx, JobKind::OutreachSequence).await.unwrap();
    let contact = db::contacts::by_email(&ctx.pool, &email).await.unwrap().unwrap();
    assert_eq!(contact.status, ContactStatus::InSequence);
    let state = db::sequences::for_contact(&ctx.pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_step, 0);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("outreach"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "subject": "Scaling the AE ramp",
                "body": "Hi Casey,\n\nSaw the AE hiring push. Worth a quick chat about ramp time?\n\nBest",
                "personalization_fact": "they are hiring several AEs",
            }))),
        )
        .mount(&llm)
        .await;
    Mock::given(method("POST"))
        .and(path("/gmail/v1/users/me/messages/send"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "sent-1", "threadId": "thread-1",
        })))
        .expect(1..)
        .mount(&gmail)
        .await;
    let tuesday = Utc.with_ymd_and_hms(2026, 8, 18, 10, 0, 0).unwrap();
    let policies = prospecting_agent::llm::default_policies();
    let rules = MessagingRules::default();
    let inputs = SendPassInputs {
        memory: &ctx.memory,
        llm: &ctx.llm,
        policies: &policies,
        rules: &rules,
        preflight_config: &ctx.config.preflight,
        transport: ctx.gmail.as_ref(),
        dry_run: false,
        limit: 100_000,
    };
    let send = run_send_pass(&ctx.pool, &inputs, tuesday).await.unwrap();
    assert!(send.sent >= 1);
    let state = db::sequences::for_contact(&ctx.pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_step, 1);
    let engagements = db::engagements::for_contact(&ctx.pool, contact.id, 10)
        .await
        .unwrap();
    assert_eq!(engagements.len(), 1);

    let reply_id = format!("reply-{}", uuid::Uuid::new_v4().simple());
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "messages": [{"id": reply_id, "threadId": "thread-1"}],
        })))
        .mount(&gmail)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/gmail/v1/users/me/messages/{reply_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": reply_id, "threadId": "thread-1",
            "snippet": "Interesting - can you share pricing?",
            "payload": {"headers": [
                {"name": "From", "value": format!("Casey Lee <{}>", email.to_uppercase())},
                {"name": "Subject", "value": "Re: Scaling the AE ramp"},
            ]},
        })))
        .mount(&gmail)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("share pricing"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
                "intent": "interested",
                "summary": "Asked for pricing.",
                "suggested_action": "send pricing and offer a call",
                "notify_rep": true,
            }))),
        )
        .mount(&llm)
        .await;
    let monitor = run_job(&ctx, JobKind::ReplyMonitor).await.unwrap();
    assert_eq!(monitor["replies_processed"], 1);
    let contact = db::contacts::by_email(&ctx.pool, &email).await.unwrap().unwrap();
    assert_eq!(contact.status, ContactStatus::Replied);
    let state = db::sequences::for_contact(&ctx.pool, contact.id)
        .await
        .unwrap()
        .unwrap();
    assert!(state.stopped);

    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "answer": "They are hiring.",
            "results": [{"title": "Careers", "url": format!("https://{domain}/careers"),
                          "content": "Sales roles open", "score": 0.9, "published_date": null}],
        })))
        .mount(&tavily)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("web_result"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion_with(&serde_json::json!({
            "signals": [{"kind": "hiring", "strength": "strong",
                          "summary": "Several AE openings", "source_url": format!("https://{domain}/careers")}],
        }))))
        .mount(&llm)
        .await;
    let signals = run_job(&ctx, JobKind::DetectSignals).await.unwrap();
    assert!(signals["signals_ingested"].as_u64().unwrap() >= 1);

    let report = run_job(&ctx, JobKind::WeeklyReport).await.unwrap();
    assert!(report["emails_sent"].as_i64().unwrap() >= 1);
    assert!(report["replies"].as_i64().unwrap() >= 1);
    assert!(report["contacts_added"].as_i64().unwrap() >= 1);
}
