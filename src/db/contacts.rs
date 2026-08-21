use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::{Contact, ContactStatus};

fn from_row(row: &PgRow) -> Result<Contact, DbError> {
    let seniority = row
        .get::<Option<String>, _>("seniority")
        .map(|s| enum_from_str(&s, "contact.seniority"))
        .transpose()?;
    Ok(Contact {
        id: row.get("id"),
        email: row.get("email"),
        first_name: row.get("first_name"),
        last_name: row.get("last_name"),
        title: row.get("title"),
        seniority,
        linkedin_url: row.get("linkedin_url"),
        company_domain: row.get("company_domain"),
        crm_id: row.get("crm_id"),
        source: enum_from_str(&row.get::<String, _>("source"), "contact.source")?,
        score: row
            .get::<Option<i16>, _>("score")
            .map(|s| {
                u8::try_from(s).map_err(|_| DbError::Codec {
                    context: "contact.score",
                    reason: format!("{s} out of range"),
                })
            })
            .transpose()?,
        status: enum_from_str(&row.get::<String, _>("status"), "contact.status")?,
        assigned_sender: row.get("assigned_sender"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

const UPDATE_CLAUSE: &str = "first_name = EXCLUDED.first_name, \
     last_name = EXCLUDED.last_name, title = EXCLUDED.title, seniority = EXCLUDED.seniority, \
     linkedin_url = EXCLUDED.linkedin_url, company_domain = EXCLUDED.company_domain, \
     crm_id = EXCLUDED.crm_id, source = EXCLUDED.source, score = EXCLUDED.score, \
     status = EXCLUDED.status, assigned_sender = EXCLUDED.assigned_sender, \
     updated_at = EXCLUDED.updated_at";

async fn resolve_id(pool: &PgPool, contact: &Contact) -> Result<Uuid, DbError> {
    let Some(email) = &contact.email else {
        return Ok(contact.id);
    };
    let existing = sqlx::query_scalar::<_, Uuid>("SELECT id FROM contacts WHERE email = $1")
        .bind(crate::domain::normalize_email(email))
        .fetch_optional(pool)
        .await?;
    Ok(existing.unwrap_or(contact.id))
}

pub async fn upsert(pool: &PgPool, contact: &Contact) -> Result<Uuid, DbError> {
    let seniority = contact
        .seniority
        .as_ref()
        .map(|s| enum_to_str(s, "contact.seniority"))
        .transpose()?;
    let source = enum_to_str(&contact.source, "contact.source")?;
    let status = enum_to_str(&contact.status, "contact.status")?;
    let sql = format!(
        "INSERT INTO contacts (id, email, first_name, last_name, title, seniority, linkedin_url, \
         company_domain, crm_id, source, score, status, assigned_sender, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
         ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, {UPDATE_CLAUSE} \
         RETURNING id"
    );
    let mut retried = false;
    loop {
        let id = resolve_id(pool, contact).await?;
        let result = sqlx::query(&sql)
            .bind(id)
            .bind(contact.email.as_deref().map(crate::domain::normalize_email))
            .bind(&contact.first_name)
            .bind(&contact.last_name)
            .bind(&contact.title)
            .bind(seniority.as_deref())
            .bind(&contact.linkedin_url)
            .bind(
                contact
                    .company_domain
                    .as_deref()
                    .map(crate::domain::normalize_domain),
            )
            .bind(&contact.crm_id)
            .bind(&source)
            .bind(contact.score.map(i16::from))
            .bind(&status)
            .bind(&contact.assigned_sender)
            .bind(contact.created_at)
            .bind(contact.updated_at)
            .fetch_one(pool)
            .await;
        match result {
            Ok(row) => return Ok(row.get("id")),
            Err(sqlx::Error::Database(db)) if !retried && db.is_unique_violation() => retried = true,
            Err(e) => return Err(e.into()),
        }
    }
}

const IMPORT_UPDATE_CLAUSE: &str = "first_name = COALESCE(EXCLUDED.first_name, contacts.first_name), \
     last_name = COALESCE(EXCLUDED.last_name, contacts.last_name), \
     title = COALESCE(EXCLUDED.title, contacts.title), \
     linkedin_url = COALESCE(EXCLUDED.linkedin_url, contacts.linkedin_url), \
     company_domain = COALESCE(EXCLUDED.company_domain, contacts.company_domain), \
     updated_at = EXCLUDED.updated_at";

pub async fn upsert_import(pool: &PgPool, contact: &Contact) -> Result<Uuid, DbError> {
    let seniority = contact
        .seniority
        .as_ref()
        .map(|s| enum_to_str(s, "contact.seniority"))
        .transpose()?;
    let source = enum_to_str(&contact.source, "contact.source")?;
    let status = enum_to_str(&contact.status, "contact.status")?;
    let sql = format!(
        "INSERT INTO contacts (id, email, first_name, last_name, title, seniority, linkedin_url, \
         company_domain, crm_id, source, score, status, assigned_sender, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
         ON CONFLICT (id) DO UPDATE SET {IMPORT_UPDATE_CLAUSE} \
         RETURNING id"
    );
    let mut retried = false;
    loop {
        let id = resolve_id(pool, contact).await?;
        let result = sqlx::query(&sql)
            .bind(id)
            .bind(contact.email.as_deref().map(crate::domain::normalize_email))
            .bind(&contact.first_name)
            .bind(&contact.last_name)
            .bind(&contact.title)
            .bind(seniority.as_deref())
            .bind(&contact.linkedin_url)
            .bind(
                contact
                    .company_domain
                    .as_deref()
                    .map(crate::domain::normalize_domain),
            )
            .bind(&contact.crm_id)
            .bind(&source)
            .bind(contact.score.map(i16::from))
            .bind(&status)
            .bind(&contact.assigned_sender)
            .bind(contact.created_at)
            .bind(contact.updated_at)
            .fetch_one(pool)
            .await;
        match result {
            Ok(row) => return Ok(row.get("id")),
            Err(sqlx::Error::Database(db)) if !retried && db.is_unique_violation() => retried = true,
            Err(e) => return Err(e.into()),
        }
    }
}

pub async fn by_id(pool: &PgPool, id: Uuid) -> Result<Option<Contact>, DbError> {
    let row = sqlx::query("SELECT * FROM contacts WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}

const ENRICH_UPDATE_CLAUSE: &str = "email = COALESCE(contacts.email, EXCLUDED.email), \
     first_name = COALESCE(EXCLUDED.first_name, contacts.first_name), \
     last_name = COALESCE(EXCLUDED.last_name, contacts.last_name), \
     title = COALESCE(EXCLUDED.title, contacts.title), \
     seniority = COALESCE(EXCLUDED.seniority, contacts.seniority), \
     linkedin_url = COALESCE(EXCLUDED.linkedin_url, contacts.linkedin_url), \
     company_domain = COALESCE(EXCLUDED.company_domain, contacts.company_domain), \
     crm_id = COALESCE(EXCLUDED.crm_id, contacts.crm_id), \
     updated_at = EXCLUDED.updated_at";

pub async fn upsert_enrichment(pool: &PgPool, contact: &Contact) -> Result<Uuid, DbError> {
    let seniority = contact
        .seniority
        .as_ref()
        .map(|s| enum_to_str(s, "contact.seniority"))
        .transpose()?;
    let source = enum_to_str(&contact.source, "contact.source")?;
    let status = enum_to_str(&contact.status, "contact.status")?;
    let sql = format!(
        "INSERT INTO contacts (id, email, first_name, last_name, title, seniority, linkedin_url, \
         company_domain, crm_id, source, score, status, assigned_sender, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
         ON CONFLICT (id) DO UPDATE SET {ENRICH_UPDATE_CLAUSE} \
         RETURNING id"
    );
    let mut retried = false;
    loop {
        let id = resolve_id(pool, contact).await?;
        let result = sqlx::query(&sql)
            .bind(id)
            .bind(contact.email.as_deref().map(crate::domain::normalize_email))
            .bind(&contact.first_name)
            .bind(&contact.last_name)
            .bind(&contact.title)
            .bind(seniority.as_deref())
            .bind(&contact.linkedin_url)
            .bind(
                contact
                    .company_domain
                    .as_deref()
                    .map(crate::domain::normalize_domain),
            )
            .bind(&contact.crm_id)
            .bind(&source)
            .bind(contact.score.map(i16::from))
            .bind(&status)
            .bind(&contact.assigned_sender)
            .bind(contact.created_at)
            .bind(contact.updated_at)
            .fetch_one(pool)
            .await;
        match result {
            Ok(row) => return Ok(row.get("id")),
            Err(sqlx::Error::Database(db)) if !retried && db.is_unique_violation() => retried = true,
            Err(e) => return Err(e.into()),
        }
    }
}

pub async fn set_crm_id(pool: &PgPool, id: Uuid, crm_id: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE contacts SET crm_id = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(crm_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn advance_status(
    pool: &PgPool,
    id: Uuid,
    from: ContactStatus,
    to: ContactStatus,
) -> Result<bool, DbError> {
    let result =
        sqlx::query("UPDATE contacts SET status = $3, updated_at = now() WHERE id = $1 AND status = $2")
            .bind(id)
            .bind(enum_to_str(&from, "contact.status")?)
            .bind(enum_to_str(&to, "contact.status")?)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn by_crm_id(pool: &PgPool, crm_id: &str) -> Result<Option<Contact>, DbError> {
    let row = sqlx::query("SELECT * FROM contacts WHERE crm_id = $1")
        .bind(crm_id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn by_email(pool: &PgPool, email: &str) -> Result<Option<Contact>, DbError> {
    let row = sqlx::query("SELECT * FROM contacts WHERE email = $1")
        .bind(crate::domain::normalize_email(email))
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn list_by_status(
    pool: &PgPool,
    status: ContactStatus,
    limit: i64,
) -> Result<Vec<Contact>, DbError> {
    let rows = sqlx::query("SELECT * FROM contacts WHERE status = $1 ORDER BY updated_at DESC LIMIT $2")
        .bind(enum_to_str(&status, "contact.status")?)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter().map(from_row).collect()
}

pub async fn list_by_company_domain(pool: &PgPool, domain: &str) -> Result<Vec<Contact>, DbError> {
    let rows = sqlx::query("SELECT * FROM contacts WHERE company_domain = $1 ORDER BY updated_at DESC")
        .bind(crate::domain::normalize_domain(domain))
        .fetch_all(pool)
        .await?;
    rows.iter().map(from_row).collect()
}

pub async fn set_status<'e>(
    pool: impl sqlx::PgExecutor<'e>,
    id: Uuid,
    status: ContactStatus,
) -> Result<(), DbError> {
    sqlx::query("UPDATE contacts SET status = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(enum_to_str(&status, "contact.status")?)
        .execute(pool)
        .await?;
    Ok(())
}
