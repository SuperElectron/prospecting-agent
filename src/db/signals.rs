use sqlx::{PgPool, Row, postgres::PgRow};

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::Signal;

fn from_row(row: &PgRow) -> Result<Signal, DbError> {
    Ok(Signal {
        id: row.get("id"),
        company_domain: row.get("company_domain"),
        kind: enum_from_str(&row.get::<String, _>("kind"), "signal.kind")?,
        strength: enum_from_str(&row.get::<String, _>("strength"), "signal.strength")?,
        summary: row.get("summary"),
        source_url: row.get("source_url"),
        detected_at: row.get("detected_at"),
    })
}

pub async fn insert(pool: &PgPool, s: &Signal) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO signals (id, company_domain, kind, strength, summary, source_url, detected_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(s.id)
    .bind(&s.company_domain)
    .bind(enum_to_str(&s.kind, "signal.kind")?)
    .bind(enum_to_str(&s.strength, "signal.strength")?)
    .bind(&s.summary)
    .bind(&s.source_url)
    .bind(s.detected_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn for_domain(pool: &PgPool, domain: &str, limit: i64) -> Result<Vec<Signal>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM signals WHERE company_domain = $1 ORDER BY detected_at DESC LIMIT $2")
            .bind(domain)
            .bind(limit)
            .fetch_all(pool)
            .await?;
    rows.iter().map(from_row).collect()
}
