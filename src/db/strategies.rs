use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row, postgres::PgRow};

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::AccountStrategy;

fn from_row(row: &PgRow) -> Result<AccountStrategy, DbError> {
    let flags: serde_json::Value = row.get("coordination_flags");
    let coordination_flags = flags
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(AccountStrategy {
        domain: row.get("domain"),
        stage: enum_from_str(&row.get::<String, _>("stage"), "strategy.stage")?,
        health: enum_from_str(&row.get::<String, _>("health"), "strategy.health")?,
        coordination_flags,
        summary: row.get("summary"),
        updated_at: row.get::<DateTime<Utc>, _>("updated_at"),
    })
}

pub async fn upsert(pool: &PgPool, strategy: &AccountStrategy) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO account_strategies (domain, stage, health, coordination_flags, summary, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (domain) DO UPDATE SET stage = EXCLUDED.stage, health = EXCLUDED.health, \
         coordination_flags = EXCLUDED.coordination_flags, summary = EXCLUDED.summary, \
         updated_at = EXCLUDED.updated_at",
    )
    .bind(crate::domain::normalize_domain(&strategy.domain))
    .bind(enum_to_str(&strategy.stage, "strategy.stage")?)
    .bind(enum_to_str(&strategy.health, "strategy.health")?)
    .bind(serde_json::json!(strategy.coordination_flags))
    .bind(&strategy.summary)
    .bind(strategy.updated_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn by_domain(pool: &PgPool, domain: &str) -> Result<Option<AccountStrategy>, DbError> {
    let row = sqlx::query("SELECT * FROM account_strategies WHERE domain = $1")
        .bind(crate::domain::normalize_domain(domain))
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}
