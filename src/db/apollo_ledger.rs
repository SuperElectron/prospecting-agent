use chrono::NaiveDate;
use sqlx::PgPool;

use crate::db::DbError;

pub async fn spent_on(pool: &PgPool, day: NaiveDate) -> Result<i32, DbError> {
    let spent: Option<i32> = sqlx::query_scalar("SELECT credits FROM apollo_spend WHERE day = $1")
        .bind(day)
        .fetch_optional(pool)
        .await?;
    Ok(spent.unwrap_or(0))
}

pub async fn record_spend(pool: &PgPool, day: NaiveDate, credits: u16) -> Result<(), DbError> {
    if credits == 0 {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO apollo_spend (day, credits) VALUES ($1, $2) \
         ON CONFLICT (day) DO UPDATE SET credits = apollo_spend.credits + EXCLUDED.credits",
    )
    .bind(day)
    .bind(i32::from(credits))
    .execute(pool)
    .await?;
    Ok(())
}
