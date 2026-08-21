use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::{AgentTask, TaskStatus};

fn from_row(row: &PgRow) -> Result<AgentTask, DbError> {
    Ok(AgentTask {
        id: row.get("id"),
        kind: enum_from_str(&row.get::<String, _>("kind"), "task.kind")?,
        contact_id: row.get("contact_id"),
        company_domain: row.get("company_domain"),
        payload: row.get("payload"),
        status: enum_from_str(&row.get::<String, _>("status"), "task.status")?,
        attempts: u8::try_from(row.get::<i16, _>("attempts")).map_err(|_| DbError::Codec {
            context: "task.attempts",
            reason: "out of range".into(),
        })?,
        due_at: row.get("due_at"),
        created_at: row.get("created_at"),
    })
}

pub async fn insert(pool: &PgPool, task: &AgentTask) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO agent_tasks (id, kind, contact_id, company_domain, payload, status, attempts, \
         due_at, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(task.id)
    .bind(enum_to_str(&task.kind, "task.kind")?)
    .bind(task.contact_id)
    .bind(&task.company_domain)
    .bind(&task.payload)
    .bind(enum_to_str(&task.status, "task.status")?)
    .bind(i16::from(task.attempts))
    .bind(task.due_at)
    .bind(task.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn claim_due(pool: &PgPool, now: DateTime<Utc>, limit: i64) -> Result<Vec<AgentTask>, DbError> {
    let rows = sqlx::query(
        "UPDATE agent_tasks SET status = 'running', attempts = attempts + 1, claimed_at = now() \
         WHERE id IN (SELECT id FROM agent_tasks \
                      WHERE (status = 'pending' AND (due_at IS NULL OR due_at <= $1)) \
                         OR (status = 'running' AND claimed_at < now() - interval '1 hour') \
                      ORDER BY created_at ASC LIMIT $2 FOR UPDATE SKIP LOCKED) \
         RETURNING *",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter().map(from_row).collect()
}

pub async fn finish(pool: &PgPool, id: Uuid, status: TaskStatus) -> Result<(), DbError> {
    sqlx::query("UPDATE agent_tasks SET status = $2 WHERE id = $1")
        .bind(id)
        .bind(enum_to_str(&status, "task.status")?)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn reschedule(pool: &PgPool, id: Uuid, due_at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE agent_tasks SET status = 'pending', due_at = $2 WHERE id = $1")
        .bind(id)
        .bind(due_at)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn by_id(pool: &PgPool, id: Uuid) -> Result<Option<AgentTask>, DbError> {
    let row = sqlx::query("SELECT * FROM agent_tasks WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}
