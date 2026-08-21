pub mod audit;
pub mod capacity;
pub mod companies;
pub mod contacts;
pub mod engagements;
pub mod sequences;
pub mod signals;
pub mod strategies;
pub mod tasks;

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub use audit::{AuditEntry, PropertyChange};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("codec error for {context}: {reason}")]
    Codec { context: &'static str, reason: String },
}

pub async fn connect(database_url: &str) -> Result<PgPool, DbError> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(database_url)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub fn enum_to_str<T: Serialize>(value: &T, context: &'static str) -> Result<String, DbError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        Ok(other) => Err(DbError::Codec {
            context,
            reason: format!("expected string encoding, got {other}"),
        }),
        Err(e) => Err(DbError::Codec {
            context,
            reason: e.to_string(),
        }),
    }
}

pub fn enum_from_str<T: DeserializeOwned>(raw: &str, context: &'static str) -> Result<T, DbError> {
    serde_json::from_value(serde_json::Value::String(raw.to_string())).map_err(|e| DbError::Codec {
        context,
        reason: format!("'{raw}': {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContactStatus, SignalStrength};

    #[test]
    fn enum_codec_round_trips_snake_case() {
        let s = enum_to_str(&ContactStatus::InSequence, "status").unwrap();
        assert_eq!(s, "in_sequence");
        let back: ContactStatus = enum_from_str(&s, "status").unwrap();
        assert_eq!(back, ContactStatus::InSequence);
    }

    #[test]
    fn enum_codec_rejects_unknown_variants() {
        let r: Result<SignalStrength, _> = enum_from_str("colossal", "strength");
        assert!(r.is_err());
    }
}
