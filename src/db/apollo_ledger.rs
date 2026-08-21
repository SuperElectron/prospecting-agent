use chrono::NaiveDate;
use sqlx::PgPool;

use crate::db::DbError;

pub async fn reserve(pool: &PgPool, day: NaiveDate, want: u16, cap: u16) -> Result<u16, DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO apollo_spend (day, credits) VALUES ($1, 0) ON CONFLICT (day) DO NOTHING")
        .bind(day)
        .execute(&mut *tx)
        .await?;
    let spent: i32 = sqlx::query_scalar("SELECT credits FROM apollo_spend WHERE day = $1 FOR UPDATE")
        .bind(day)
        .fetch_one(&mut *tx)
        .await?;
    let remaining = (i32::from(cap) - spent).max(0);
    let granted = u16::try_from(remaining.min(i32::from(want))).unwrap_or(0);
    sqlx::query("UPDATE apollo_spend SET credits = credits + $2 WHERE day = $1")
        .bind(day)
        .bind(i32::from(granted))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(granted)
}

pub async fn refund(pool: &PgPool, day: NaiveDate, unused: u16) -> Result<(), DbError> {
    if unused == 0 {
        return Ok(());
    }
    sqlx::query("UPDATE apollo_spend SET credits = greatest(credits - $2, 0) WHERE day = $1")
        .bind(day)
        .bind(i32::from(unused))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn spent_on(pool: &PgPool, day: NaiveDate) -> Result<i32, DbError> {
    let spent: Option<i32> = sqlx::query_scalar("SELECT credits FROM apollo_spend WHERE day = $1")
        .bind(day)
        .fetch_optional(pool)
        .await?;
    Ok(spent.unwrap_or(0))
}
