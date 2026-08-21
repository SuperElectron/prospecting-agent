use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use crate::db::DbError;

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyChange {
    pub entity_type: String,
    pub entity_id: String,
    pub property: String,
    pub old_value: Option<serde_json::Value>,
    pub new_value: serde_json::Value,
    pub confidence: Option<f32>,
    pub updated_by: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub change: PropertyChange,
    pub changed_at: DateTime<Utc>,
}

pub async fn record(pool: &PgPool, change: &PropertyChange) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO audit_history (entity_type, entity_id, property, old_value, new_value, \
         confidence, updated_by) VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&change.entity_type)
    .bind(&change.entity_id)
    .bind(&change.property)
    .bind(&change.old_value)
    .bind(&change.new_value)
    .bind(change.confidence)
    .bind(&change.updated_by)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn history(
    pool: &PgPool,
    entity_type: &str,
    entity_id: &str,
    limit: i64,
) -> Result<Vec<AuditEntry>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM audit_history WHERE entity_type = $1 AND entity_id = $2 \
         ORDER BY changed_at DESC LIMIT $3",
    )
    .bind(entity_type)
    .bind(entity_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| AuditEntry {
            change: PropertyChange {
                entity_type: row.get("entity_type"),
                entity_id: row.get("entity_id"),
                property: row.get("property"),
                old_value: row.get("old_value"),
                new_value: row.get("new_value"),
                confidence: row.get("confidence"),
                updated_by: row.get("updated_by"),
            },
            changed_at: row.get("changed_at"),
        })
        .collect())
}
