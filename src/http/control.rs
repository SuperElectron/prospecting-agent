use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::connectors::CheckStatus;
use crate::db::{contacts, engagements, sequences, strategies};
use crate::http::AppState;
use crate::jobs::{self, JobError, JobKind, runtime};
use crate::workflows::reporting;

const RUN_TIMEOUT: Duration = Duration::from_secs(600);

pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let report = jobs::health_report(&state.ctx).await;
    let status = if report.status == CheckStatus::Error {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (status, axum::Json(serde_json::json!(report)))
}

pub async fn list_jobs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let names: Vec<&str> = JobKind::ALL.iter().map(|kind| kind.as_str()).collect();
    axum::Json(serde_json::json!({
        "jobs": names,
        "webhooks": state.webhooks.names(),
    }))
}

pub async fn run_job_now(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> impl IntoResponse {
    let Ok(kind) = JobKind::from_str(&name) else {
        return unknown_job(&name);
    };
    {
        let mut running = state.running.lock().await;
        if !running.insert(kind) {
            return (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({"error": format!("job {kind} is already running")})),
            );
        }
    }
    let outcome = tokio::time::timeout(RUN_TIMEOUT, jobs::run_job(&state.ctx, kind)).await;
    state.running.lock().await.remove(&kind);
    match outcome {
        Ok(Ok(report)) => (StatusCode::OK, axum::Json(report)),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        ),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            axum::Json(serde_json::json!({
                "error": format!("job {kind} timed out after {}s", RUN_TIMEOUT.as_secs())
            })),
        ),
    }
}

pub async fn enqueue_job(State(state): State<Arc<AppState>>, Path(name): Path<String>) -> impl IntoResponse {
    let Ok(kind) = JobKind::from_str(&name) else {
        return unknown_job(&name);
    };
    match runtime::enqueue(&state.queue, kind).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            axum::Json(serde_json::json!({"enqueued": kind.as_str()})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn report(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match reporting::weekly_report(&state.ctx.pool, 7).await {
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
    let contact = match contacts::by_email(pool, &email).await {
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
    let engagements = engagements::for_contact(pool, contact.id, 10)
        .await
        .unwrap_or_default();
    let sequence = sequences::for_contact(pool, contact.id).await.ok().flatten();
    let strategy = match contact.company_domain.as_deref() {
        Some(domain) => strategies::by_domain(pool, domain).await.ok().flatten(),
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

fn unknown_job(name: &str) -> (StatusCode, axum::Json<serde_json::Value>) {
    let error = JobError::Unknown(name.to_string());
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({"error": error.to_string()})),
    )
}
