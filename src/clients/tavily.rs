use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::Secret;

const DEFAULT_BASE_URL: &str = "https://api.tavily.com";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_ATTEMPTS: u32 = 3;
const DEFAULT_MAX_RESULTS: u8 = 5;
const DEFAULT_RECENCY_DAYS: u16 = 30;

#[derive(Debug, thiserror::Error)]
pub enum TavilyError {
    #[error("tavily request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("tavily returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("tavily response shape unexpected: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDepth {
    Basic,
    Advanced,
}

impl SearchDepth {
    fn as_str(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Advanced => "advanced",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub max_results: u8,
    pub depth: SearchDepth,
    pub recency_days: u16,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            depth: SearchDepth::Basic,
            recency_days: DEFAULT_RECENCY_DAYS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub score: f64,
    pub published_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone)]
pub struct TavilyClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Secret,
}

impl TavilyClient {
    pub fn new(api_key: Secret) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: Secret, base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("static tavily client configuration is valid");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub async fn search(&self, query: &str, options: &SearchOptions) -> Result<SearchResponse, TavilyError> {
        let url = format!("{}/search", self.base_url);
        let body = json!({
            "query": query,
            "search_depth": options.depth.as_str(),
            "max_results": options.max_results,
            "include_answer": true,
            "include_raw_content": false,
            "days": options.recency_days,
        });
        let mut last_error: Option<TavilyError> = None;
        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
            }
            let response = self
                .http
                .post(&url)
                .bearer_auth(self.api_key.expose())
                .json(&body)
                .send()
                .await;
            match response {
                Ok(resp) if resp.status().is_success() => {
                    let raw = resp.text().await?;
                    return serde_json::from_str(&raw).map_err(|e| TavilyError::Decode(e.to_string()));
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    let error = TavilyError::Status { status, body };
                    if status == 429 || (500..600).contains(&status) {
                        last_error = Some(error);
                    } else {
                        return Err(error);
                    }
                }
                Err(e) => last_error = Some(TavilyError::Http(e)),
            }
        }
        Err(last_error.unwrap_or(TavilyError::Decode("no attempts made".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> TavilyClient {
        TavilyClient::with_base_url(Secret::new("tvly-test"), &server.uri())
    }

    #[tokio::test]
    async fn search_sends_expected_body_and_bearer_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("authorization", "Bearer tvly-test"))
            .and(body_partial_json(serde_json::json!({
                "query": "Acme funding",
                "search_depth": "basic",
                "max_results": 5,
                "include_answer": true,
                "days": 30,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "answer": "Acme raised a Series B.",
                "results": [{"title": "Acme raises", "url": "https://news.example.com/acme",
                             "content": "Acme announced...", "score": 0.93,
                             "published_date": "2026-08-01"}],
            })))
            .expect(1)
            .mount(&server)
            .await;
        let response = client(&server)
            .search("Acme funding", &SearchOptions::default())
            .await
            .unwrap();
        assert_eq!(response.answer.as_deref(), Some("Acme raised a Series B."));
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].published_date.as_deref(), Some("2026-08-01"));
    }

    #[tokio::test]
    async fn rate_limits_are_retried_then_succeed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"answer": null, "results": []})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let response = client(&server)
            .search("q", &SearchOptions::default())
            .await
            .unwrap();
        assert!(response.results.is_empty());
        assert!(response.answer.is_none());
    }

    #[tokio::test]
    async fn client_errors_are_typed_and_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .expect(1)
            .mount(&server)
            .await;
        let err = client(&server)
            .search("q", &SearchOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, TavilyError::Status { status: 401, .. }));
    }

    #[tokio::test]
    async fn advanced_depth_and_custom_knobs_reach_the_wire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_partial_json(serde_json::json!({
                "search_depth": "advanced",
                "max_results": 8,
                "days": 90,
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"answer": null, "results": []})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let options = SearchOptions {
            max_results: 8,
            depth: SearchDepth::Advanced,
            recency_days: 90,
        };
        client(&server).search("q", &options).await.unwrap();
    }
}
