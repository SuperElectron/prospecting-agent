use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::env::LlmConfig;

const MAX_ATTEMPTS: u32 = 3;
const REQUEST_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("llm request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("llm returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("llm response had no choices")]
    EmptyResponse,
    #[error("llm output parse failure: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    temperature: f32,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

#[derive(Debug, Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmClient {
    pub fn new(config: &LlmConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.expose().to_string(),
            model: config.model.clone(),
        }
    }

    pub async fn chat(&self, messages: &[ChatMessage]) -> Result<String, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let request = ChatRequest {
            model: &self.model,
            messages,
            temperature: 0.4,
        };
        let mut last_error: Option<LlmError> = None;
        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(300 * u64::from(attempt))).await;
            }
            let response = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&request)
                .send()
                .await;
            match response {
                Ok(resp) if resp.status().is_success() => {
                    let parsed: ChatResponse = resp.json().await?;
                    return parsed
                        .choices
                        .into_iter()
                        .next()
                        .map(|c| c.message.content)
                        .ok_or(LlmError::EmptyResponse);
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    let error = LlmError::Status { status, body };
                    if (500..600).contains(&status) {
                        last_error = Some(error);
                    } else {
                        return Err(error);
                    }
                }
                Err(e) => last_error = Some(LlmError::Http(e)),
            }
        }
        Err(last_error.unwrap_or(LlmError::EmptyResponse))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::secret::Secret;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config(base_url: String) -> LlmConfig {
        LlmConfig {
            base_url,
            api_key: Secret::new("test"),
            model: "gpt-oss-120b".into(),
        }
    }

    fn completion(content: &str) -> serde_json::Value {
        serde_json::json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
    }

    #[tokio::test]
    async fn chat_returns_first_choice_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion("hello")))
            .mount(&server)
            .await;
        let client = LlmClient::new(&config(format!("{}/v1", server.uri())));
        let out = client.chat(&[ChatMessage::user("hi")]).await.unwrap();
        assert_eq!(out, "hello");
    }

    #[tokio::test]
    async fn chat_retries_transient_server_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion("recovered")))
            .mount(&server)
            .await;
        let client = LlmClient::new(&config(format!("{}/v1", server.uri())));
        let out = client.chat(&[ChatMessage::user("hi")]).await.unwrap();
        assert_eq!(out, "recovered");
    }

    #[tokio::test]
    async fn chat_does_not_retry_client_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let client = LlmClient::new(&config(format!("{}/v1", server.uri())));
        let err = client.chat(&[ChatMessage::user("hi")]).await.unwrap_err();
        assert!(matches!(err, LlmError::Status { status: 401, .. }));
    }
}
