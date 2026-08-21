pub mod control;
pub mod inbound;
pub mod webhooks;

use std::collections::HashSet;
use std::sync::Arc;

use apalis_sql::postgres::PostgresStorage;
use axum::Router;
use axum::routing::{get, post};

use crate::http::webhooks::WebhookRegistry;
use crate::jobs::{JobContext, JobKind, NamedJob};

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("cannot bind control api on port {port}: {source}")]
    Bind { port: u16, source: std::io::Error },
    #[error("control api stopped: {0}")]
    Serve(#[from] std::io::Error),
}

pub struct AppState {
    pub ctx: JobContext,
    pub queue: PostgresStorage<NamedJob>,
    pub webhooks: WebhookRegistry,
    pub running: std::sync::Mutex<HashSet<JobKind>>,
}

impl AppState {
    pub fn new(ctx: JobContext, queue: PostgresStorage<NamedJob>, webhooks: WebhookRegistry) -> Self {
        Self {
            ctx,
            queue,
            webhooks,
            running: std::sync::Mutex::new(HashSet::new()),
        }
    }
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

pub async fn serve(state: Arc<AppState>, port: u16) -> Result<(), HttpError> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|source| HttpError::Bind { port, source })?;
    tracing::info!(port, "control api listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        })
        .await?;
    Ok(())
}
