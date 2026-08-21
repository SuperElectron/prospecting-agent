pub mod control;
pub mod webhooks;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::http::webhooks::WebhookRegistry;
use crate::jobs::JobContext;

pub struct AppState {
    pub ctx: JobContext,
    pub webhooks: WebhookRegistry,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(control::health))
        .route("/control/jobs", get(control::list_jobs))
        .route("/control/jobs/{name}/run", post(control::run_job_now))
        .route("/control/jobs/{name}/enqueue", post(control::enqueue_job))
        .route("/control/report", get(control::report))
        .route("/control/contacts/{email}", get(control::contact_lookup))
        .route("/webhooks/{name}", post(webhooks::dispatch))
        .with_state(state)
}

pub async fn serve(state: Arc<AppState>, port: u16) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    tracing::info!(port, "control api listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        })
        .await?;
    Ok(())
}
