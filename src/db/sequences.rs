use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::SequenceState;

fn from_row(row: &PgRow) -> Result<SequenceState, DbError> {
    let stop_reason = row
        .get::<Option<String>, _>("stop_reason")
        .map(|s| enum_from_str(&s, "sequence.stop_reason"))
        .transpose()?;
    Ok(SequenceState {
        contact_id: row.get("contact_id"),
        cadence: row.get("cadence"),
        current_step: u8::try_from(row.get::<i16, _>("current_step")).map_err(|_| DbError::Codec {
            context: "sequence.current_step",
            reason: "out of range".into(),
        })?,
        max_steps: u8::try_from(row.get::<i16, _>("max_steps")).map_err(|_| DbError::Codec {
            context: "sequence.max_steps",
            reason: "out of range".into(),
        })?,
        last_sent_at: row.get("last_sent_at"),
        stopped: row.get("stopped"),
        stop_reason,
        stopped_at: row.get("stopped_at"),
    })
}

pub async fn upsert<'e>(pool: impl sqlx::PgExecutor<'e>, state: &SequenceState) -> Result<(), DbError> {
    let stop_reason = state
        .stop_reason
        .as_ref()
        .map(|r| enum_to_str(r, "sequence.stop_reason"))
        .transpose()?;
    sqlx::query(
        "INSERT INTO sequence_states (contact_id, cadence, current_step, max_steps, last_sent_at, \
         stopped, stop_reason, stopped_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
         ON CONFLICT (contact_id) DO UPDATE SET cadence = EXCLUDED.cadence, \
         current_step = EXCLUDED.current_step, max_steps = EXCLUDED.max_steps, \
         last_sent_at = EXCLUDED.last_sent_at, stopped = EXCLUDED.stopped, \
         stop_reason = EXCLUDED.stop_reason, stopped_at = EXCLUDED.stopped_at",
    )
    .bind(state.contact_id)
    .bind(&state.cadence)
    .bind(i16::from(state.current_step))
    .bind(i16::from(state.max_steps))
    .bind(state.last_sent_at)
    .bind(state.stopped)
    .bind(stop_reason)
    .bind(state.stopped_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn for_contact(pool: &PgPool, contact_id: Uuid) -> Result<Option<SequenceState>, DbError> {
    let row = sqlx::query("SELECT * FROM sequence_states WHERE contact_id = $1")
        .bind(contact_id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn list_active(pool: &PgPool, limit: i64) -> Result<Vec<SequenceState>, DbError> {
    let rows = sqlx::query("SELECT * FROM sequence_states WHERE stopped = false LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter().map(from_row).collect()
}
