use chrono::{Duration, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::connectors::{ConnectorError, Notifier, NotifyLevel};
use crate::db;
use crate::domain::{AgentTask, ContactStatus, TaskKind, TaskStatus};
use crate::jobs::JobContext;
use crate::workflows::sync::ingest_person;

const CLAIM_LIMIT: i64 = 20;
const TASK_CREDITS: u16 = 5;
const MAX_TASK_ATTEMPTS: u8 = 3;
const RETRY_DELAY_HOURS: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("task carries no {0}")]
    Missing(&'static str),
    #[error("contact {0} not found")]
    ContactNotFound(Uuid),
    #[error("storage error: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("apollo error: {0}")]
    Apollo(#[from] crate::clients::apollo::ApolloError),
    #[error("ingest error: {0}")]
    Ingest(#[from] crate::workflows::sync::SyncError),
    #[error("research error: {0}")]
    Research(#[from] crate::workflows::research::ResearchError),
    #[error("notify error: {0}")]
    Notify(#[from] ConnectorError),
}

enum Completion {
    Done,
    Skipped,
    Deferred,
}

#[derive(Debug, Default, Serialize)]
pub struct TaskRunReport {
    pub claimed: u64,
    pub done: u64,
    pub skipped: u64,
    pub retried: u64,
    pub failed: u64,
    pub deferred: u64,
    pub credits_spent: u64,
    pub credit_capped: bool,
    pub bookkeeping_failed: u64,
}

pub async fn execute_due(ctx: &JobContext) -> Result<TaskRunReport, crate::jobs::JobError> {
    let mut report = TaskRunReport::default();
    let reservation = crate::jobs::reserve_apollo_credits(ctx, TASK_CREDITS).await?;
    report.credit_capped = reservation.capped;
    let mut credits = reservation.granted;
    let tasks = db::tasks::claim_due(&ctx.pool, Utc::now(), CLAIM_LIMIT).await?;
    for task in tasks {
        report.claimed += 1;
        let before = credits;
        let outcome = execute_one(ctx, &task, &mut credits).await;
        report.credits_spent += u64::from(before - credits);
        let bookkeeping = match outcome {
            Ok(Completion::Done) => {
                report.done += 1;
                db::tasks::finish(&ctx.pool, task.id, TaskStatus::Done).await
            }
            Ok(Completion::Skipped) => {
                report.skipped += 1;
                db::tasks::finish(&ctx.pool, task.id, TaskStatus::Skipped).await
            }
            Ok(Completion::Deferred) => {
                report.deferred += 1;
                let due = Utc::now() + Duration::hours(RETRY_DELAY_HOURS);
                db::tasks::defer(&ctx.pool, task.id, due).await
            }
            Err(reason) => {
                tracing::warn!(task = %task.id, kind = ?task.kind, error = %reason, "task attempt failed");
                if task.attempts >= MAX_TASK_ATTEMPTS {
                    report.failed += 1;
                    db::tasks::finish(&ctx.pool, task.id, TaskStatus::Failed).await
                } else {
                    report.retried += 1;
                    let due = Utc::now() + Duration::hours(RETRY_DELAY_HOURS);
                    db::tasks::reschedule(&ctx.pool, task.id, due).await
                }
            }
        };
        if let Err(e) = bookkeeping {
            tracing::warn!(task = %task.id, error = %e, "task bookkeeping failed; reaper will reclaim");
            report.bookkeeping_failed += 1;
        }
    }
    crate::jobs::settle_apollo_credits(ctx, &reservation, credits).await?;
    Ok(report)
}

async fn execute_one(ctx: &JobContext, task: &AgentTask, credits: &mut u16) -> Result<Completion, TaskError> {
    match task.kind {
        TaskKind::EnrichContact => enrich_contact(ctx, task, credits).await,
        TaskKind::ResearchCompany => research_company(ctx, task).await,
        TaskKind::NotifyRep => notify_rep(ctx, task).await,
        TaskKind::GenerateOutreach | TaskKind::SendOutreach | TaskKind::AnalyzeReply | TaskKind::SyncCrm => {
            Ok(Completion::Skipped)
        }
    }
}

async fn enrich_contact(
    ctx: &JobContext,
    task: &AgentTask,
    credits: &mut u16,
) -> Result<Completion, TaskError> {
    if *credits == 0 {
        return Ok(Completion::Deferred);
    }
    let contact_id = task.contact_id.ok_or(TaskError::Missing("contact id"))?;
    let contact = db::contacts::by_id(&ctx.pool, contact_id)
        .await?
        .ok_or(TaskError::ContactNotFound(contact_id))?;
    let email = contact
        .email
        .as_deref()
        .ok_or(TaskError::Missing("contact email"))?;
    *credits -= 1;
    let Some(person) = ctx.apollo.match_person(email).await? else {
        db::contacts::advance_status(&ctx.pool, contact_id, ContactStatus::New, ContactStatus::NoMatch)
            .await?;
        return Ok(Completion::Done);
    };
    let enriched_id = ingest_person(&ctx.pool, &ctx.memory, &person).await?;
    if enriched_id != contact_id {
        tracing::warn!(
            queried = %contact_id,
            resolved = %enriched_id,
            "enrich task resolved to a different contact"
        );
        db::contacts::advance_status(&ctx.pool, contact_id, ContactStatus::New, ContactStatus::NoMatch)
            .await?;
    }
    db::contacts::advance_status(
        &ctx.pool,
        enriched_id,
        ContactStatus::New,
        ContactStatus::Enriched,
    )
    .await?;
    Ok(Completion::Done)
}

async fn research_company(ctx: &JobContext, task: &AgentTask) -> Result<Completion, TaskError> {
    let domain = task
        .company_domain
        .as_deref()
        .ok_or(TaskError::Missing("company domain"))?;
    crate::workflows::research::research_company(
        &ctx.pool,
        &ctx.memory,
        &ctx.tavily,
        &ctx.llm,
        &ctx.policies,
        domain,
    )
    .await?;
    Ok(Completion::Done)
}

async fn notify_rep(ctx: &JobContext, task: &AgentTask) -> Result<Completion, TaskError> {
    let message = task
        .payload
        .get("message")
        .and_then(|value| value.as_str())
        .ok_or(TaskError::Missing("message"))?;
    ctx.notifier.notify(NotifyLevel::Info, message).await?;
    Ok(Completion::Done)
}
