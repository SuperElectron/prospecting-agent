use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::env::MemoryConfig;
use crate::memory::entities::EntityRef;

const REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_ATTEMPTS: u32 = 3;
const MAX_PAGES: u32 = 5;
const PAGE_SIZE: usize = 50;
const APP_NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("memory service returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("memory backend unavailable: {0}")]
    Backend(String),
    #[error("memory response shape unexpected: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub content: String,
    pub created_at: i64,
    #[serde(default, rename = "metadata_")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct FilterResponse {
    items: Vec<MemoryItem>,
    total: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MemoryClient {
    http: reqwest::Client,
    base_url: String,
    user_id: String,
}

impl MemoryClient {
    pub fn new(config: &MemoryConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("static memory client configuration is valid");
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            user_id: config.user.clone(),
        }
    }

    async fn post_checked(&self, url: &str, body: &serde_json::Value) -> Result<String, MemoryError> {
        let mut last_error: Option<MemoryError> = None;
        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
            }
            match self.http.post(url).json(body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let response_body = resp.text().await?;
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&response_body)
                        && let Some(error) = value.get("error").and_then(|e| e.as_str())
                    {
                        return Err(MemoryError::Backend(error.to_string()));
                    }
                    return Ok(response_body);
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    let error = MemoryError::Status { status, body };
                    if status == 429 || (500..600).contains(&status) {
                        last_error = Some(error);
                    } else {
                        return Err(error);
                    }
                }
                Err(e) => last_error = Some(MemoryError::Http(e)),
            }
        }
        Err(last_error.unwrap_or(MemoryError::Decode("no attempts made".into())))
    }

    pub async fn memorize(&self, entity: &EntityRef, text: &str, infer: bool) -> Result<(), MemoryError> {
        let url = format!("{}/api/v1/memories/", self.base_url);
        let body = json!({
            "user_id": self.user_id,
            "text": text,
            "metadata": {"entity": entity.tag()},
            "infer": infer,
            "app": APP_NAME,
        });
        self.post_checked(&url, &body).await?;
        Ok(())
    }

    pub async fn recall(
        &self,
        query: &str,
        entity: Option<&EntityRef>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, MemoryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let url = format!("{}/api/v1/memories/filter", self.base_url);
        let wanted_tag = entity.map(EntityRef::tag);
        let page_size = if wanted_tag.is_some() {
            PAGE_SIZE
        } else {
            limit.min(PAGE_SIZE)
        };
        let mut collected: Vec<MemoryItem> = Vec::new();
        for page in 1..=MAX_PAGES {
            let body = json!({
                "user_id": self.user_id,
                "search_query": query,
                "size": page_size,
                "page": page,
                "sort_column": "created_at",
                "sort_direction": "desc",
            });
            let response_body = self.post_checked(&url, &body).await?;
            let parsed: FilterResponse =
                serde_json::from_str(&response_body).map_err(|e| MemoryError::Decode(e.to_string()))?;
            let fetched = parsed.items.len();
            collected.extend(parsed.items.into_iter().filter(|item| {
                match &wanted_tag {
                    None => true,
                    Some(tag) => item
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("entity"))
                        .and_then(|e| e.as_str())
                        .is_some_and(|e| e == tag),
                }
            }));
            if collected.len() >= limit || fetched < page_size {
                break;
            }
            if let Some(total) = parsed.total
                && u64::try_from(page_size).unwrap_or(u64::MAX) * u64::from(page) >= total
            {
                break;
            }
        }
        collected.truncate(limit);
        Ok(collected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config(base_url: &str) -> MemoryConfig {
        MemoryConfig {
            base_url: base_url.to_string(),
            user: "prospecting".into(),
        }
    }

    fn item(content: &str, entity: &str, created_at: i64) -> serde_json::Value {
        json!({
            "id": Uuid::new_v4().to_string(),
            "content": content,
            "created_at": created_at,
            "state": "active",
            "app_id": Uuid::new_v4().to_string(),
            "app_name": APP_NAME,
            "categories": [],
            "metadata_": {"entity": entity},
        })
    }

    fn page(items: &[serde_json::Value], total: u64, page_no: u32) -> serde_json::Value {
        json!({"items": items, "total": total, "page": page_no, "size": 50, "pages": total.div_ceil(50)})
    }

    #[tokio::test]
    async fn memorize_posts_entity_tagged_payload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/"))
            .and(body_partial_json(json!({
                "user_id": "prospecting",
                "text": "raised series B",
                "metadata": {"entity": "company:acme.io"},
                "app": APP_NAME,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "m1"})))
            .expect(1)
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let entity = EntityRef::company("Acme.io");
        client.memorize(&entity, "raised series B", false).await.unwrap();
    }

    #[tokio::test]
    async fn backend_error_in_success_body_is_surfaced_on_both_paths() {
        let server = MockServer::start().await;
        let error_body = json!({"error": "Memory client is not available"});
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(error_body.clone()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(200).set_body_json(error_body))
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let entity = EntityRef::company("acme.io");
        let write_err = client.memorize(&entity, "x", true).await.unwrap_err();
        assert!(matches!(write_err, MemoryError::Backend(_)));
        let read_err = client.recall("q", None, 5).await.unwrap_err();
        assert!(matches!(read_err, MemoryError::Backend(_)));
    }

    #[tokio::test]
    async fn recall_filters_by_entity_and_asserts_request_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .and(body_partial_json(json!({
                "user_id": "prospecting",
                "search_query": "acme",
                "size": 50,
                "page": 1,
                "sort_column": "created_at",
                "sort_direction": "desc",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(
                &[
                    item("acme fact one", "company:acme.io", 300),
                    item("other company fact", "company:other.io", 200),
                    item("acme fact two", "company:acme.io", 100),
                ],
                3,
                1,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let entity = EntityRef::company("acme.io");
        let items = client.recall("acme", Some(&entity), 1).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "acme fact one");
    }

    #[tokio::test]
    async fn recall_pages_forward_when_entity_matches_are_sparse() {
        let server = MockServer::start().await;
        let page_one: Vec<serde_json::Value> = (0..50)
            .map(|i| item("noise", "company:other.io", 1000 - i))
            .collect();
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .and(body_partial_json(json!({"page": 1})))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&page_one, 51, 1)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .and(body_partial_json(json!({"page": 2})))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(
                &[item("the buried fact", "company:acme.io", 1)],
                51,
                2,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let entity = EntityRef::company("acme.io");
        let items = client.recall("q", Some(&entity), 3).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "the buried fact");
    }

    #[tokio::test]
    async fn zero_limit_short_circuits_without_a_request() {
        let server = MockServer::start().await;
        let client = MemoryClient::new(&config(&server.uri()));
        let items = client.recall("q", None, 0).await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn transient_errors_are_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&[], 0, 1)))
            .expect(1)
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let items = client.recall("q", None, 5).await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn http_error_status_is_typed_and_not_retried_for_client_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(404).set_body_string("{\"detail\":\"User not found\"}"))
            .expect(1)
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let err = client.recall("q", None, 5).await.unwrap_err();
        assert!(matches!(err, MemoryError::Status { status: 404, .. }));
    }

    #[tokio::test]
    async fn unexpected_shape_is_a_decode_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"unexpected": true})))
            .mount(&server)
            .await;
        let client = MemoryClient::new(&config(&server.uri()));
        let err = client.recall("q", None, 5).await.unwrap_err();
        assert!(matches!(err, MemoryError::Decode(_)));
    }
}
