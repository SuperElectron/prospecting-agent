use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::connectors::is_plausible_email;
use crate::db;
use crate::domain::{Company, Contact, ContactSource};
use crate::memory::{EntityRef, MemoryClient, MemoryError};
use crate::workflows::sync::SyncError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowSkip {
    pub file: String,
    pub line: u64,
    pub reason: String,
}

#[derive(Debug, Default, Serialize)]
pub struct CsvSyncReport {
    pub contacts_upserted: u64,
    pub companies_upserted: u64,
    pub notes_memorized: u64,
    pub skipped: Vec<RowSkip>,
}

#[derive(Deserialize)]
struct CompanyRow {
    domain: String,
    name: String,
    industry: String,
    employee_count: String,
    location: String,
}

#[derive(Deserialize)]
struct ContactRow {
    email: String,
    first_name: String,
    last_name: String,
    title: String,
    company_domain: String,
    linkedin_url: String,
}

#[derive(Deserialize)]
struct NoteRow {
    company_domain: String,
    note: String,
    noted_at: String,
}

pub async fn sync_dir(pool: &PgPool, memory: &MemoryClient, dir: &str) -> Result<CsvSyncReport, SyncError> {
    let mut report = CsvSyncReport::default();
    sync_companies(pool, dir, &mut report).await?;
    sync_contacts(pool, dir, &mut report).await?;
    sync_notes(memory, dir, &mut report).await?;
    Ok(report)
}

fn reader(dir: &str, file: &str) -> Result<Option<csv::Reader<std::fs::File>>, SyncError> {
    let path = Path::new(dir).join(file);
    if !path.exists() {
        return Ok(None);
    }
    csv::Reader::from_path(&path)
        .map(Some)
        .map_err(|e| SyncError::Io {
            path: path.display().to_string(),
            reason: e.to_string(),
        })
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

async fn sync_companies(pool: &PgPool, dir: &str, report: &mut CsvSyncReport) -> Result<(), SyncError> {
    let Some(mut rows) = reader(dir, "companies.csv")? else {
        return Ok(());
    };
    for (index, row) in rows.deserialize::<CompanyRow>().enumerate() {
        let line = index as u64 + 2;
        let skip = |reason: String| RowSkip {
            file: "companies.csv".into(),
            line,
            reason,
        };
        let row = match row {
            Ok(row) => row,
            Err(e) => {
                report.skipped.push(skip(e.to_string()));
                continue;
            }
        };
        let mut company = Company::new(&row.domain);
        if company.domain.is_empty() {
            report.skipped.push(skip("empty domain".into()));
            continue;
        }
        let raw_count = non_empty(&row.employee_count);
        let employee_count = if let Some(raw) = &raw_count {
            let Ok(count) = raw.parse::<u32>() else {
                report
                    .skipped
                    .push(skip(format!("employee_count {raw:?} is not a number")));
                continue;
            };
            Some(count)
        } else {
            None
        };
        company.name = non_empty(&row.name);
        company.industry = non_empty(&row.industry);
        company.employee_count = employee_count;
        company.location = non_empty(&row.location);
        db::companies::upsert(pool, &company).await?;
        report.companies_upserted += 1;
    }
    Ok(())
}

async fn sync_contacts(pool: &PgPool, dir: &str, report: &mut CsvSyncReport) -> Result<(), SyncError> {
    let Some(mut rows) = reader(dir, "contacts.csv")? else {
        return Ok(());
    };
    for (index, row) in rows.deserialize::<ContactRow>().enumerate() {
        let line = index as u64 + 2;
        let skip = |reason: String| RowSkip {
            file: "contacts.csv".into(),
            line,
            reason,
        };
        let row = match row {
            Ok(row) => row,
            Err(e) => {
                report.skipped.push(skip(e.to_string()));
                continue;
            }
        };
        let email = row.email.trim();
        if !is_plausible_email(email) {
            report.skipped.push(skip(format!("implausible email {email:?}")));
            continue;
        }
        let mut contact = Contact::new(ContactSource::Csv);
        contact.email = Some(email.to_string());
        contact.first_name = non_empty(&row.first_name);
        contact.last_name = non_empty(&row.last_name);
        contact.title = non_empty(&row.title);
        contact.company_domain = non_empty(&row.company_domain);
        contact.linkedin_url = non_empty(&row.linkedin_url);
        db::contacts::upsert(pool, &contact).await?;
        report.contacts_upserted += 1;
    }
    Ok(())
}

async fn sync_notes(memory: &MemoryClient, dir: &str, report: &mut CsvSyncReport) -> Result<(), SyncError> {
    let Some(mut rows) = reader(dir, "notes.csv")? else {
        return Ok(());
    };
    for (index, row) in rows.deserialize::<NoteRow>().enumerate() {
        let line = index as u64 + 2;
        let skip = |reason: String| RowSkip {
            file: "notes.csv".into(),
            line,
            reason,
        };
        let row = match row {
            Ok(row) => row,
            Err(e) => {
                report.skipped.push(skip(e.to_string()));
                continue;
            }
        };
        let entity = EntityRef::company(&row.company_domain);
        let Some(note) = non_empty(&row.note) else {
            report.skipped.push(skip("empty note".into()));
            continue;
        };
        let dated = match non_empty(&row.noted_at) {
            Some(date) => format!("[{date}] {note}"),
            None => note,
        };
        match memory.memorize(&entity, &dated, false).await {
            Ok(()) => report.notes_memorized += 1,
            Err(MemoryError::Backend(reason)) => report.skipped.push(skip(reason)),
            Err(other) => return Err(other.into()),
        }
    }
    Ok(())
}
