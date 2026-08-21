use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use apalis::prelude::*;
use apalis_sql::postgres::PostgresStorage;
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;

use crate::connectors::{Notifier, NotifyLevel};
use crate::jobs::{JobContext, JobError, JobKind, NamedJob, run_job};

const DEFAULT_SCHEDULES: [(JobKind, &str); 9] = [
    (JobKind::CsvSync, "0 0 6 * * *"),
    (JobKind::EnrichContacts, "0 30 6 * * *"),
    (JobKind::DiscoverContacts, "0 45 6 * * *"),
    (JobKind::ResearchCompanies, "0 0 7 * * *"),
    (JobKind::DetectSignals, "0 0 8 * * *"),
    (JobKind::OutreachSequence, "0 15 7 * * *"),
    (JobKind::OutreachSend, "0 10 * * * *"),
    (JobKind::TaskExecutor, "0 5,35 * * * *"),
    (JobKind::WeeklyReport, "0 0 9 * * Mon"),
];

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("queue storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("invalid cron expression {expression}: {reason}")]
    Cron { expression: String, reason: String },
    #[error("worker runtime error: {0}")]
    Monitor(#[from] std::io::Error),
}

async fn execute(job: NamedJob, ctx: Data<Arc<JobContext>>) -> Result<(), Error> {
    tracing::info!(job = %job.name, "job starting");
    match run_job(&ctx, job.name).await {
        Ok(report) => {
            tracing::info!(job = %job.name, %report, "job finished");
            Ok(())
        }
        Err(e) => {
            let message = format!("job {name} failed: {e}", name = job.name);
            if let Err(notify_error) = ctx.notifier.notify(NotifyLevel::Error, &message).await {
                tracing::warn!(error = %notify_error, "job failure notify failed");
            }
            let boxed: BoxDynError = Box::new(e);
            Err(boxed.into())
        }
    }
}

pub async fn queue_pool(database_url: &str) -> Result<sqlx::PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET search_path = apalis_meta, apalis, public")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await?;
    sqlx::query("CREATE SCHEMA IF NOT EXISTS apalis_meta")
        .execute(&pool)
        .await?;
    Ok(pool)
}

pub async fn storage(database_url: &str) -> Result<PostgresStorage<NamedJob>, sqlx::Error> {
    let pool = queue_pool(database_url).await?;
    PostgresStorage::setup(&pool).await?;
    Ok(PostgresStorage::new(pool))
}

pub async fn enqueue(backend: &PostgresStorage<NamedJob>, kind: JobKind) -> Result<(), JobError> {
    let mut backend = backend.clone();
    backend.push(NamedJob { name: kind }).await?;
    Ok(())
}

fn spawn_schedules(backend: &PostgresStorage<NamedJob>) -> Result<(), RuntimeError> {
    for (kind, expression) in DEFAULT_SCHEDULES {
        let schedule = apalis_cron::Schedule::from_str(expression).map_err(|e| RuntimeError::Cron {
            expression: expression.to_string(),
            reason: e.to_string(),
        })?;
        let backend = backend.clone();
        tokio::spawn(async move {
            for next in schedule.upcoming_owned(Utc) {
                match (next - Utc::now()).to_std() {
                    Ok(wait) => tokio::time::sleep(wait).await,
                    Err(_) => {
                        tracing::warn!(job = %kind, "cron occurrence already elapsed; firing now");
                    }
                }
                if let Err(e) = enqueue(&backend, kind).await {
                    tracing::warn!(job = %kind, error = %e, "cron enqueue failed");
                } else {
                    tracing::info!(job = %kind, "cron enqueued");
                }
            }
        });
    }
    Ok(())
}

pub async fn run_worker(ctx: JobContext) -> Result<(), RuntimeError> {
    let backend = storage(ctx.config.database_url.expose()).await?;
    spawn_schedules(&backend)?;
    let shared = Arc::new(ctx);
    let queue_worker = WorkerBuilder::new("named-jobs")
        .data(shared)
        .backend(backend)
        .build_fn(execute);
    tracing::info!(
        schedules = DEFAULT_SCHEDULES.len(),
        "worker starting: queue plus cron schedules"
    );
    Monitor::new().register(queue_worker).run().await?;
    Ok(())
}
