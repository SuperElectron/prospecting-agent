use prospecting_agent::clients::{ApolloClient, PeopleSearchParams, SearchOptions, TavilyClient};
use prospecting_agent::config::Secret;

#[tokio::test]
#[ignore = "live paid smoke; run with --ignored and TEST_APOLLO_LIVE_KEY"]
async fn live_apollo_people_search_smoke() {
    let Ok(key) = std::env::var("TEST_APOLLO_LIVE_KEY") else {
        eprintln!("TEST_APOLLO_LIVE_KEY not set; skipping apollo live smoke");
        return;
    };
    let client = ApolloClient::new(Secret::new(key));
    let params = PeopleSearchParams {
        organization_domains: vec!["stripe.com".into()],
        person_seniorities: vec!["vp".into()],
        per_page: Some(1),
        ..PeopleSearchParams::default()
    };
    let response = client.search_people(&params).await.unwrap();
    assert!(!response.people.is_empty());
    assert!(response.total_entries.unwrap_or(0) > 0);
}

#[tokio::test]
#[ignore = "live paid smoke; run with --ignored and TEST_TAVILY_LIVE_KEY"]
async fn live_tavily_search_smoke() {
    let Ok(key) = std::env::var("TEST_TAVILY_LIVE_KEY") else {
        eprintln!("TEST_TAVILY_LIVE_KEY not set; skipping tavily live smoke");
        return;
    };
    let client = TavilyClient::new(Secret::new(key));
    let response = client
        .search("Stripe payments company news", &SearchOptions::default())
        .await
        .unwrap();
    assert!(!response.results.is_empty());
}
