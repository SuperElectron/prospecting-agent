use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::Engagement;

fn from_row(row: &PgRow) -> Result<Engagement, DbError> {
    Ok(Engagement {
        id: row.get("id"),
        contact_id: row.get("contact_id"),
        channel: enum_from_str(&row.get::<String, _>("channel"), "engagement.channel")?,
        direction: enum_from_str(&row.get::<String, _>("direction"), "engagement.direction")?,
        kind: enum_from_str(&row.get::<String, _>("kind"), "engagement.kind")?,
        subject: row.get("subject"),
        body: row.get("body"),
        sequence_step: row
            .get::<Option<i16>, _>("sequence_step")
            .map(|s| {
                u8::try_from(s).map_err(|_| DbError::Codec {
                    context: "engagement.sequence_step",
                    reason: format!("{s} out of range"),
                })
            })
            .transpose()?,
        occurred_at: row.get("occurred_at"),
    })
}

pub async fn insert(pool: &PgPool, e: &Engagement) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO engagements (id, contact_id, channel, direction, kind, subject, body, \
         sequence_step, occurred_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING",
    )
    .bind(e.id)
    .bind(e.contact_id)
    .bind(enum_to_str(&e.channel, "engagement.channel")?)
    .bind(enum_to_str(&e.direction, "engagement.direction")?)
    .bind(enum_to_str(&e.kind, "engagement.kind")?)
    .bind(&e.subject)
    .bind(&e.body)
    .bind(e.sequence_step.map(i16::from))
    .bind(e.occurred_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn for_contact(pool: &PgPool, contact_id: Uuid, limit: i64) -> Result<Vec<Engagement>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM engagements WHERE contact_id = $1 ORDER BY occurred_at DESC LIMIT $2")
            .bind(contact_id)
            .bind(limit)
            .fetch_all(pool)
            .await?;
    rows.iter().map(from_row).collect()
}
