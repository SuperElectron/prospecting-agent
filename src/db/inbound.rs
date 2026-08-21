use sqlx::PgPool;
use uuid::Uuid;

use crate::db::DbError;

pub async fn claim_message(
    pool: &PgPool,
    gmail_message_id: &str,
    sender_email: &str,
) -> Result<bool, DbError> {
    let result = sqlx::query(
        "INSERT INTO processed_inbound (gmail_message_id, sender_email) VALUES ($1, $2) \
         ON CONFLICT (gmail_message_id) DO UPDATE \
         SET processed_at = now(), sender_email = EXCLUDED.sender_email \
         WHERE processed_inbound.completed_at IS NULL \
           AND processed_inbound.processed_at < now() - interval '1 hour'",
    )
    .bind(gmail_message_id)
    .bind(sender_email)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_completed(pool: &PgPool, gmail_message_id: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE processed_inbound SET completed_at = now() WHERE gmail_message_id = $1")
        .bind(gmail_message_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn release_message(pool: &PgPool, gmail_message_id: &str) -> Result<(), DbError> {
    sqlx::query("DELETE FROM processed_inbound WHERE gmail_message_id = $1")
        .bind(gmail_message_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn attach_contact(pool: &PgPool, gmail_message_id: &str, contact_id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE processed_inbound SET contact_id = $2 WHERE gmail_message_id = $1")
        .bind(gmail_message_id)
        .bind(contact_id)
        .execute(pool)
        .await?;
    Ok(())
}
