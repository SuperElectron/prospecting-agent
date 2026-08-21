use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::http::AppState;

pub type WebhookResult = Result<serde_json::Value, String>;

type Handler = Arc<
    dyn Fn(Arc<AppState>, serde_json::Value) -> std::pin::Pin<Box<dyn Future<Output = WebhookResult> + Send>>
        + Send
        + Sync,
>;

#[derive(Default, Clone)]
pub struct WebhookRegistry {
    handlers: HashMap<String, Handler>,
}

impl WebhookRegistry {
    pub fn register<F, Fut>(&mut self, name: &str, handler: F)
    where
        F: Fn(Arc<AppState>, serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = WebhookResult> + Send + 'static,
    {
        self.handlers.insert(
            name.to_string(),
            Arc::new(move |state, payload| Box::pin(handler(state, payload))),
        );
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.handlers.keys().cloned().collect();
        names.sort();
        names
    }

    fn get(&self, name: &str) -> Option<Handler> {
        self.handlers.get(name).cloned()
    }
}

pub async fn dispatch(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(handler) = state.webhooks.get(&name) else {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": format!("no webhook named {name}")})),
        );
    };
    match handler(state.clone(), payload).await {
        Ok(result) => (StatusCode::OK, axum::Json(result)),
        Err(reason) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({"error": reason})),
        ),
    }
}
