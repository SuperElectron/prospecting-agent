use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::Secret;
use crate::domain;

fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

const DEFAULT_BASE_URL: &str = "https://api.apollo.io";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_ATTEMPTS: u32 = 3;
const DEFAULT_PER_PAGE: u8 = 25;
const MAX_PER_PAGE: u8 = 100;

#[derive(Debug, thiserror::Error)]
pub enum ApolloError {
    #[error("apollo request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("apollo returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("apollo response shape unexpected: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ApolloPerson {
    #[serde(default, deserialize_with = "null_to_default")]
    pub id: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub title: Option<String>,
    pub email: Option<String>,
    pub email_status: Option<String>,
    pub linkedin_url: Option<String>,
    pub seniority: Option<String>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub departments: Vec<String>,
    pub organization: Option<ApolloOrganization>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ApolloOrganization {
    #[serde(default, deserialize_with = "null_to_default")]
    pub id: String,
    pub name: Option<String>,
    pub website_url: Option<String>,
    pub primary_domain: Option<String>,
    pub industry: Option<String>,
    pub estimated_num_employees: Option<u32>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub linkedin_url: Option<String>,
    pub founded_year: Option<u16>,
    #[serde(default, deserialize_with = "null_to_default")]
    pub keywords: Vec<String>,
    pub short_description: Option<String>,
    pub latest_funding_stage: Option<String>,
    pub total_funding_printed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PeopleSearchResponse {
    #[serde(default)]
    pub people: Vec<ApolloPerson>,
    pub total_entries: Option<u64>,
}

#[derive(Deserialize)]
struct MatchResponse {
    person: Option<ApolloPerson>,
}

#[derive(Deserialize)]
struct OrgEnrichResponse {
    organization: Option<ApolloOrganization>,
}

#[derive(Debug, Clone, Default)]
pub struct PeopleSearchParams {
    pub organization_domains: Vec<String>,
    pub person_titles: Vec<String>,
    pub person_seniorities: Vec<String>,
    pub person_departments: Vec<String>,
    pub per_page: Option<u8>,
    pub page: Option<u32>,
}

impl PeopleSearchParams {
    fn body(&self) -> serde_json::Value {
        let mut body = json!({
            "per_page": self.per_page.unwrap_or(DEFAULT_PER_PAGE).min(MAX_PER_PAGE),
            "page": self.page.unwrap_or(1),
        });
        let extras = [
            ("q_organization_domains_list", &self.organization_domains),
            ("person_titles", &self.person_titles),
            ("person_seniorities", &self.person_seniorities),
            ("person_departments", &self.person_departments),
        ];
        for (key, values) in extras {
            if !values.is_empty() {
                body[key] = json!(values);
            }
        }
        body
    }
}

#[derive(Debug, Clone)]
pub struct ApolloClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Secret,
}

impl ApolloClient {
    pub fn new(api_key: Secret) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: Secret, base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("static apollo client configuration is valid");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    async fn post_json(&self, endpoint: &str, body: &serde_json::Value) -> Result<String, ApolloError> {
        let url = format!("{}{endpoint}", self.base_url);
        let mut attempt = 0;
        loop {
            if attempt > 0 {
                tracing::warn!(attempt, %url, "retrying apollo request");
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
            }
            let response = self
                .http
                .post(&url)
                .header("X-Api-Key", self.api_key.expose())
                .json(body)
                .send()
                .await;
            let error = match response {
                Ok(resp) if resp.status().is_success() => return Ok(resp.text().await?),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    let error = ApolloError::Status { status, body };
                    if status == 429 || (500..600).contains(&status) {
                        error
                    } else {
                        return Err(error);
                    }
                }
                Err(e) => ApolloError::Http(e),
            };
            attempt += 1;
            if attempt >= MAX_ATTEMPTS {
                return Err(error);
            }
        }
    }

    fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, ApolloError> {
        serde_json::from_str(raw).map_err(|e| ApolloError::Decode(e.to_string()))
    }

    pub async fn search_people(
        &self,
        params: &PeopleSearchParams,
    ) -> Result<PeopleSearchResponse, ApolloError> {
        let raw = self
            .post_json("/v1/mixed_people/api_search", &params.body())
            .await?;
        Self::decode(&raw)
    }

    pub async fn match_person(&self, email: &str) -> Result<Option<ApolloPerson>, ApolloError> {
        let body = json!({"email": email, "reveal_personal_emails": false});
        let raw = self.post_json("/v1/people/match", &body).await?;
        let parsed: MatchResponse = Self::decode(&raw)?;
        Ok(parsed.person)
    }

    pub async fn enrich_organization(&self, domain: &str) -> Result<Option<ApolloOrganization>, ApolloError> {
        let body = json!({"domain": domain::normalize_domain(domain)});
        let raw = self.post_json("/v1/organizations/enrich", &body).await?;
        let parsed: OrgEnrichResponse = Self::decode(&raw)?;
        Ok(parsed.organization)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> ApolloClient {
        ApolloClient::with_base_url(Secret::new("apollo-test"), &server.uri())
    }

    fn person_json() -> serde_json::Value {
        serde_json::json!({
            "id": "p-1", "first_name": "Jane", "last_name": "Doe", "title": "VP Sales",
            "email": "jane@acme.io", "email_status": "verified", "seniority": "vp",
            "departments": ["sales"], "linkedin_url": "https://linkedin.com/in/jane",
            "organization": {"id": "o-1", "name": "Acme", "primary_domain": "acme.io"},
        })
    }

    #[tokio::test]
    async fn search_people_sends_api_key_and_filters() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/mixed_people/api_search"))
            .and(header("x-api-key", "apollo-test"))
            .and(body_partial_json(serde_json::json!({
                "q_organization_domains_list": ["acme.io"],
                "person_seniorities": ["vp", "director"],
                "per_page": 25,
                "page": 1,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "people": [person_json()],
                "total_entries": 1,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let params = PeopleSearchParams {
            organization_domains: vec!["acme.io".into()],
            person_seniorities: vec!["vp".into(), "director".into()],
            ..PeopleSearchParams::default()
        };
        let response = client(&server).search_people(&params).await.unwrap();
        assert_eq!(response.people.len(), 1);
        assert_eq!(response.people[0].email.as_deref(), Some("jane@acme.io"));
        assert_eq!(response.total_entries, Some(1));
    }

    #[tokio::test]
    async fn empty_filter_lists_are_omitted_from_the_body() {
        let params = PeopleSearchParams {
            organization_domains: vec!["acme.io".into()],
            ..PeopleSearchParams::default()
        };
        let body = params.body();
        assert!(body.get("person_titles").is_none());
        assert!(body.get("person_seniorities").is_none());
        assert!(body.get("person_departments").is_none());
        let unscoped = PeopleSearchParams::default().body();
        assert!(unscoped.get("q_organization_domains_list").is_none());
    }

    #[tokio::test]
    async fn per_page_is_clamped_to_the_apollo_maximum() {
        let params = PeopleSearchParams {
            per_page: Some(250),
            ..PeopleSearchParams::default()
        };
        assert_eq!(params.body()["per_page"], 100);
    }

    #[tokio::test]
    async fn persistent_server_errors_exhaust_exactly_max_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/people/match"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;
        let err = client(&server).match_person("jane@acme.io").await.unwrap_err();
        assert!(matches!(err, ApolloError::Status { status: 503, .. }));
    }

    #[tokio::test]
    async fn match_person_returns_none_when_apollo_has_no_person() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/people/match"))
            .and(body_partial_json(
                serde_json::json!({"reveal_personal_emails": false}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"person": null})))
            .expect(1)
            .mount(&server)
            .await;
        let found = client(&server).match_person("ghost@nowhere.io").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn enrich_organization_normalizes_the_domain_before_sending() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/organizations/enrich"))
            .and(body_partial_json(serde_json::json!({"domain": "acme.io"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"organization": {"id": "o-1", "primary_domain": "acme.io"}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let org = client(&server)
            .enrich_organization("https://www.Acme.IO/about")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(org.primary_domain.as_deref(), Some("acme.io"));
    }

    #[tokio::test]
    async fn rate_limit_is_retried_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/people/match"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/people/match"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"person": person_json()})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let found = client(&server).match_person("jane@acme.io").await.unwrap();
        assert_eq!(found.unwrap().id, "p-1");
    }

    #[tokio::test]
    async fn client_errors_are_typed_and_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/organizations/enrich"))
            .respond_with(ResponseTemplate::new(422).set_body_string("unprocessable"))
            .expect(1)
            .mount(&server)
            .await;
        let err = client(&server).enrich_organization("acme.io").await.unwrap_err();
        assert!(matches!(err, ApolloError::Status { status: 422, .. }));
    }
}
