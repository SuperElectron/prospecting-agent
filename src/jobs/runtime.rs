use std::str::FromStr;
use std::sync::Arc;

use apalis::prelude::*;
use apalis_sql::postgres::PostgresStorage;
use chrono::Utc;

use crate::jobs::{JobContext, JobError, NamedJob, run_job};

const DEFAULT_SCHEDULES: [(&str, &str); 5] = [
    ("csv_sync", "0 0 6 * * *"),
    ("enrich_contacts", "0 30 6 * * *"),
    ("research_companies", "0 0 7 * * *"),
    ("detect_signals", "0 0 8 * * *"),
    ("weekly_report", "0 0 9 * * Mon"),
];

async fn execute(job: NamedJob, ctx: Data<Arc<JobContext>>) -> Result<(), Error> {
    tracing::info!(job = job.name, "job starting");
    match run_job(&ctx, &job.name).await {
        Ok(report) => {
            tracing::info!(job = job.name, %report, "job finished");
            Ok(())
        }
        Err(e) => {
            use crate::connectors::{Notifier, NotifyLevel};
            let message = format!("job {name} failed: {e}", name = job.name);
            if let Err(notify_error) = ctx.notifier.notify(NotifyLevel::Error, &message).await {
                tracing::warn!(error = %notify_error, "job failure notify failed");
            }
            Err(Error::Failed(Arc::new(Box::new(e))))
        }
    }
}

pub async fn storage(pool: &sqlx::PgPool) -> Result<PostgresStorage<NamedJob>, sqlx::Error> {
    PostgresStorage::setup(pool).await?;
    Ok(PostgresStorage::new(pool.clone()))
}

pub async fn enqueue(pool: &sqlx::PgPool, name: &str) -> Result<(), JobError> {
    let wrap = |reason: String| JobError::Failed {
        name: name.to_string(),
        reason,
    };
    let mut backend = storage(pool).await.map_err(|e| wrap(e.to_string()))?;
    backend
        .push(NamedJob {
            name: name.to_string(),
        })
        .await
        .map_err(|e| wrap(e.to_string()))?;
    Ok(())
}

fn spawn_schedules(pool: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for (name, expression) in DEFAULT_SCHEDULES {
        let schedule = apalis_cron::Schedule::from_str(expression)?;
        let pool = pool.clone();
        tokio::spawn(async move {
            for next in schedule.upcoming_owned(Utc) {
                let wait = next - Utc::now();
                if let Ok(wait) = wait.to_std() {
                    tokio::time::sleep(wait).await;
                }
                if let Err(e) = enqueue(&pool, name).await {
                    tracing::warn!(job = name, error = %e, "cron enqueue failed");
                } else {
                    tracing::info!(job = name, "cron enqueued");
                }
            }
        });
    }
    Ok(())
}

pub async fn run_worker(ctx: JobContext) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let backend = storage(&ctx.pool).await?;
    spawn_schedules(&ctx.pool)?;
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
