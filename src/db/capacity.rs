use chrono::NaiveDate;
use sqlx::{PgPool, Row};

use crate::db::DbError;

pub async fn sent_today(pool: &PgPool, sender: &str, day: NaiveDate) -> Result<i32, DbError> {
    let row = sqlx::query("SELECT sent FROM send_capacity WHERE sender = $1 AND day = $2")
        .bind(sender)
        .bind(day)
        .fetch_optional(pool)
        .await?;
    Ok(row.map_or(0, |r| r.get("sent")))
}

pub async fn try_reserve(
    pool: &PgPool,
    sender: &str,
    day: NaiveDate,
    daily_cap: i32,
) -> Result<bool, DbError> {
    if daily_cap <= 0 {
        return Ok(false);
    }
    let row = sqlx::query(
        "INSERT INTO send_capacity (sender, day, sent) VALUES ($1, $2, 1) \
         ON CONFLICT (sender, day) DO UPDATE SET sent = send_capacity.sent + 1 \
         WHERE send_capacity.sent < $3 \
         RETURNING sent",
    )
    .bind(sender)
    .bind(day)
    .bind(daily_cap)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}
