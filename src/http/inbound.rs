use std::sync::Arc;

use crate::http::AppState;
use crate::http::webhooks::{WebhookError, WebhookRegistry};
use crate::workflows::analysis::{InboundReply, analyze_reply};

pub fn register(registry: &mut WebhookRegistry) {
    registry.register("inbound_reply", |state, payload| async move {
        handle_inbound_reply(state, payload).await
    });
}

async fn handle_inbound_reply(
    state: Arc<AppState>,
    payload: serde_json::Value,
) -> Result<serde_json::Value, WebhookError> {
    let rejected = |reason: &str| WebhookError::Rejected(reason.to_string());
    let email = payload
        .get("email")
        .and_then(|value| value.as_str())
        .ok_or_else(|| rejected("payload needs an email field"))?;
    let body = payload
        .get("body")
        .and_then(|value| value.as_str())
        .ok_or_else(|| rejected("payload needs a body field"))?;
    let subject = payload.get("subject").and_then(|value| value.as_str());
    let ctx = &state.ctx;
    let contact = crate::db::contacts::by_email(&ctx.pool, email)
        .await
        .map_err(|e| WebhookError::Internal(e.to_string()))?
        .ok_or_else(|| rejected("no contact with that email"))?;
    let outcome = analyze_reply(
        &ctx.pool,
        &ctx.memory,
        &ctx.llm,
        &ctx.policies,
        &ctx.notifier,
        &contact,
        InboundReply { subject, body },
    )
    .await
    .map_err(|e| WebhookError::Internal(e.to_string()))?;
    serde_json::to_value(&outcome).map_err(|e| WebhookError::Internal(e.to_string()))
}
