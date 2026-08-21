use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::connectors::gmail::OauthClient;
use crate::connectors::health::{DbProbe, HealthInputs, run_health_check};
use crate::http::AppState;
use crate::jobs::{JOB_NAMES, run_job};

pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ctx = &state.ctx;
    let senders = ctx
        .gmail
        .as_ref()
        .map(|g| g.senders().to_vec())
        .unwrap_or_default();
    let oauth = ctx
        .config
        .email
        .gmail
        .as_ref()
        .and_then(|g| OauthClient::from_file(&g.client_file).ok());
    let report = run_health_check(&HealthInputs {
        db: DbProbe::Pool(&ctx.pool),
        llm_base_url: Some(&ctx.config.llm.base_url),
        memory_base_url: Some(&ctx.config.memory.base_url),
        gmail: oauth.as_ref(),
        senders: &senders,
    })
    .await;
    let status = if report.status == crate::connectors::CheckStatus::Error {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (status, axum::Json(serde_json::json!(report)))
}

pub async fn list_jobs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "jobs": JOB_NAMES,
        "webhooks": state.webhooks.names(),
    }))
}

pub async fn run_job_now(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> impl IntoResponse {
    match run_job(&state.ctx, &name).await {
        Ok(report) => (StatusCode::OK, axum::Json(report)),
        Err(crate::jobs::JobError::Unknown(name)) => (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": format!("unknown job {name}")})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn enqueue_job(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> impl IntoResponse {
    if !JOB_NAMES.contains(&name.as_str()) {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": format!("unknown job {name}")})),
        );
    }
    match crate::jobs::runtime::enqueue(state.ctx.config.database_url.expose(), &name).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            axum::Json(serde_json::json!({"enqueued": name})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn report(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match crate::workflows::reporting::weekly_report(&state.ctx.pool, 7).await {
        Ok(report) => (StatusCode::OK, axum::Json(serde_json::json!(report))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn contact_lookup(
    State(state): State<Arc<AppState>>,
    Path(email): Path<String>,
) -> impl IntoResponse {
    let pool = &state.ctx.pool;
    let contact = match crate::db::contacts::by_email(pool, &email).await {
        Ok(Some(contact)) => contact,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": "no contact with that email"})),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": e.to_string()})),
            );
        }
    };
    let engagements = crate::db::engagements::for_contact(pool, contact.id, 10)
        .await
        .unwrap_or_default();
    let sequence = crate::db::sequences::for_contact(pool, contact.id)
        .await
        .ok()
        .flatten();
    let strategy = match contact.company_domain.as_deref() {
        Some(domain) => crate::db::strategies::by_domain(pool, domain)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "contact": contact,
            "engagements": engagements,
            "sequence": sequence,
            "strategy": strategy,
        })),
    )
}
