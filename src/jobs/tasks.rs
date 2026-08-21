use chrono::{Duration, Utc};
use serde::Serialize;

use crate::connectors::{Notifier, NotifyLevel};
use crate::db;
use crate::domain::{AgentTask, ContactStatus, TaskKind, TaskStatus};
use crate::jobs::{JobContext, JobError};
use crate::workflows::sync::ingest_person;

const CLAIM_LIMIT: i64 = 20;
const MAX_TASK_ATTEMPTS: u8 = 3;
const RETRY_DELAY_HOURS: i64 = 1;

#[derive(Debug, Default, Serialize)]
pub struct TaskRunReport {
    pub claimed: u64,
    pub done: u64,
    pub skipped: u64,
    pub retried: u64,
    pub failed: u64,
}

pub async fn execute_due(ctx: &JobContext) -> Result<TaskRunReport, JobError> {
    let mut report = TaskRunReport::default();
    let tasks = db::tasks::claim_due(&ctx.pool, Utc::now(), CLAIM_LIMIT).await?;
    for task in tasks {
        report.claimed += 1;
        match execute_one(ctx, &task).await {
            Ok(TaskStatus::Done) => {
                db::tasks::finish(&ctx.pool, task.id, TaskStatus::Done).await?;
                report.done += 1;
            }
            Ok(_) => {
                db::tasks::finish(&ctx.pool, task.id, TaskStatus::Skipped).await?;
                report.skipped += 1;
            }
            Err(reason) => {
                tracing::warn!(task = %task.id, kind = ?task.kind, reason, "task attempt failed");
                if task.attempts >= MAX_TASK_ATTEMPTS {
                    db::tasks::finish(&ctx.pool, task.id, TaskStatus::Failed).await?;
                    report.failed += 1;
                } else {
                    let due = Utc::now() + Duration::hours(RETRY_DELAY_HOURS);
                    db::tasks::reschedule(&ctx.pool, task.id, due).await?;
                    report.retried += 1;
                }
            }
        }
    }
    Ok(report)
}

async fn execute_one(ctx: &JobContext, task: &AgentTask) -> Result<TaskStatus, String> {
    match task.kind {
        TaskKind::EnrichContact => enrich_contact(ctx, task).await,
        TaskKind::ResearchCompany => research_company(ctx, task).await,
        TaskKind::NotifyRep => notify_rep(ctx, task).await,
        TaskKind::GenerateOutreach | TaskKind::SendOutreach | TaskKind::AnalyzeReply | TaskKind::SyncCrm => {
            Ok(TaskStatus::Skipped)
        }
    }
}

async fn enrich_contact(ctx: &JobContext, task: &AgentTask) -> Result<TaskStatus, String> {
    let Some(contact_id) = task.contact_id else {
        return Err("enrich task carries no contact id".into());
    };
    let contact = db::contacts::by_id(&ctx.pool, contact_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("contact {contact_id} not found"))?;
    let Some(email) = contact.email.as_deref() else {
        return Err(format!("contact {contact_id} has no email"));
    };
    let matched = ctx.apollo.match_person(email).await.map_err(|e| e.to_string())?;
    let Some(person) = matched else {
        db::contacts::advance_status(&ctx.pool, contact_id, ContactStatus::New, ContactStatus::NoMatch)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(TaskStatus::Done);
    };
    let enriched_id = ingest_person(&ctx.pool, &ctx.memory, &person)
        .await
        .map_err(|e| e.to_string())?;
    db::contacts::advance_status(
        &ctx.pool,
        enriched_id,
        ContactStatus::New,
        ContactStatus::Enriched,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(TaskStatus::Done)
}

async fn research_company(ctx: &JobContext, task: &AgentTask) -> Result<TaskStatus, String> {
    let Some(domain) = task.company_domain.as_deref() else {
        return Err("research task carries no company domain".into());
    };
    crate::workflows::research::research_company(
        &ctx.pool,
        &ctx.memory,
        &ctx.tavily,
        &ctx.llm,
        &ctx.policies,
        domain,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(TaskStatus::Done)
}

async fn notify_rep(ctx: &JobContext, task: &AgentTask) -> Result<TaskStatus, String> {
    let message = task
        .payload
        .get("message")
        .and_then(|value| value.as_str())
        .ok_or("notify task carries no message")?;
    ctx.notifier
        .notify(NotifyLevel::Info, message)
        .await
        .map_err(|e| e.to_string())?;
    Ok(TaskStatus::Done)
}
