use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::memory::entities::EntityRef;

const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("memory service returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("memory backend unavailable: {0}")]
    Backend(String),
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
}

#[derive(Debug, Clone)]
pub struct MemoryClient {
    http: reqwest::Client,
    base_url: String,
    user_id: String,
    app: String,
}

impl MemoryClient {
    pub fn new(base_url: &str, user_id: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("static memory client configuration is valid");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            user_id: user_id.into(),
            app: "prospecting-agent".into(),
        }
    }

    pub async fn memorize(&self, entity: &EntityRef, text: &str, infer: bool) -> Result<(), MemoryError> {
        let url = format!("{}/api/v1/memories/", self.base_url);
        let body = json!({
            "user_id": self.user_id,
            "text": text,
            "metadata": {"entity": entity.tag()},
            "infer": infer,
            "app": self.app,
        });
        let resp = self.http.post(&url).json(&body).send().await?;
        let status = resp.status().as_u16();
        let text_body = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(MemoryError::Status {
                status,
                body: text_body,
            });
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text_body)
            && let Some(error) = value.get("error").and_then(|e| e.as_str())
        {
            return Err(MemoryError::Backend(error.to_string()));
        }
        Ok(())
    }

    pub async fn recall(
        &self,
        query: &str,
        entity: Option<&EntityRef>,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, MemoryError> {
        let url = format!("{}/api/v1/memories/filter", self.base_url);
        let fetch_size = if entity.is_some() {
            limit.max(10) * 5
        } else {
            limit
        };
        let body = json!({
            "user_id": self.user_id,
            "search_query": query,
            "size": fetch_size,
            "sort_column": "created_at",
            "sort_direction": "desc",
        });
        let resp = self.http.post(&url).json(&body).send().await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(MemoryError::Status { status, body });
        }
        let parsed: FilterResponse = resp.json().await?;
        let wanted_tag = entity.map(EntityRef::tag);
        let items = parsed
            .items
            .into_iter()
            .filter(|item| match &wanted_tag {
                None => true,
                Some(tag) => item
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("entity"))
                    .and_then(|e| e.as_str())
                    .is_some_and(|e| e == tag),
            })
            .take(limit)
            .collect();
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn item(content: &str, entity: &str, created_at: i64) -> serde_json::Value {
        json!({
            "id": Uuid::new_v4().to_string(),
            "content": content,
            "created_at": created_at,
            "state": "active",
            "app_id": Uuid::new_v4().to_string(),
            "app_name": "prospecting-agent",
            "categories": [],
            "metadata_": {"entity": entity},
        })
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
                "app": "prospecting-agent",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "m1"})))
            .expect(1)
            .mount(&server)
            .await;
        let client = MemoryClient::new(&server.uri(), "prospecting");
        let entity = EntityRef::Company("acme.io".into());
        client.memorize(&entity, "raised series B", false).await.unwrap();
    }

    #[tokio::test]
    async fn backend_error_in_success_body_is_surfaced() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"error": "Memory client is not available"})),
            )
            .mount(&server)
            .await;
        let client = MemoryClient::new(&server.uri(), "prospecting");
        let entity = EntityRef::Company("acme.io".into());
        let err = client.memorize(&entity, "x", true).await.unwrap_err();
        assert!(matches!(err, MemoryError::Backend(_)));
    }

    #[tokio::test]
    async fn recall_filters_by_entity_tag_and_respects_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [
                    item("acme fact one", "company:acme.io", 300),
                    item("other company fact", "company:other.io", 200),
                    item("acme fact two", "company:acme.io", 100),
                ],
                "total": 3, "page": 1, "size": 50, "pages": 1
            })))
            .mount(&server)
            .await;
        let client = MemoryClient::new(&server.uri(), "prospecting");
        let entity = EntityRef::Company("acme.io".into());
        let items = client.recall("acme", Some(&entity), 1).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "acme fact one");
    }

    #[tokio::test]
    async fn recall_without_entity_returns_everything_up_to_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [item("a", "company:x.io", 2), item("b", "company:y.io", 1)],
                "total": 2, "page": 1, "size": 10, "pages": 1
            })))
            .mount(&server)
            .await;
        let client = MemoryClient::new(&server.uri(), "prospecting");
        let items = client.recall("q", None, 10).await.unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn http_error_status_is_typed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/memories/filter"))
            .respond_with(ResponseTemplate::new(404).set_body_string("{\"detail\":\"User not found\"}"))
            .mount(&server)
            .await;
        let client = MemoryClient::new(&server.uri(), "ghost");
        let err = client.recall("q", None, 5).await.unwrap_err();
        assert!(matches!(err, MemoryError::Status { status: 404, .. }));
    }
}
