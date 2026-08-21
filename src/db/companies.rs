use sqlx::{PgPool, Row, postgres::PgRow};

use crate::db::{DbError, enum_from_str, enum_to_str};
use crate::domain::Company;

fn from_row(row: &PgRow) -> Result<Company, DbError> {
    let hiring_velocity = row
        .get::<Option<String>, _>("hiring_velocity")
        .map(|s| enum_from_str(&s, "company.hiring_velocity"))
        .transpose()?;
    Ok(Company {
        id: row.get("id"),
        domain: row.get("domain"),
        name: row.get("name"),
        industry: row.get("industry"),
        employee_count: row
            .get::<Option<i32>, _>("employee_count")
            .and_then(|n| u32::try_from(n).ok()),
        location: row.get("location"),
        linkedin_url: row.get("linkedin_url"),
        crm_id: row.get("crm_id"),
        hiring_velocity,
        icp_fit_score: row
            .get::<Option<i16>, _>("icp_fit_score")
            .and_then(|s| u8::try_from(s).ok()),
        summary: row.get("summary"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn upsert(pool: &PgPool, company: &Company) -> Result<(), DbError> {
    let hiring_velocity = company
        .hiring_velocity
        .as_ref()
        .map(|v| enum_to_str(v, "company.hiring_velocity"))
        .transpose()?;
    sqlx::query(
        "INSERT INTO companies (id, domain, name, industry, employee_count, location, linkedin_url, \
         crm_id, hiring_velocity, icp_fit_score, summary, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
         ON CONFLICT (domain) DO UPDATE SET name = EXCLUDED.name, industry = EXCLUDED.industry, \
         employee_count = EXCLUDED.employee_count, location = EXCLUDED.location, \
         linkedin_url = EXCLUDED.linkedin_url, crm_id = EXCLUDED.crm_id, \
         hiring_velocity = EXCLUDED.hiring_velocity, icp_fit_score = EXCLUDED.icp_fit_score, \
         summary = EXCLUDED.summary, updated_at = EXCLUDED.updated_at",
    )
    .bind(company.id)
    .bind(&company.domain)
    .bind(&company.name)
    .bind(&company.industry)
    .bind(
        company
            .employee_count
            .map(|n| i32::try_from(n).unwrap_or(i32::MAX)),
    )
    .bind(&company.location)
    .bind(&company.linkedin_url)
    .bind(&company.crm_id)
    .bind(hiring_velocity)
    .bind(company.icp_fit_score.map(i16::from))
    .bind(&company.summary)
    .bind(company.created_at)
    .bind(company.updated_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn by_domain(pool: &PgPool, domain: &str) -> Result<Option<Company>, DbError> {
    let row = sqlx::query("SELECT * FROM companies WHERE domain = $1")
        .bind(domain)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn list_recent(pool: &PgPool, limit: i64) -> Result<Vec<Company>, DbError> {
    let rows = sqlx::query("SELECT * FROM companies ORDER BY updated_at DESC LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter().map(from_row).collect()
}
