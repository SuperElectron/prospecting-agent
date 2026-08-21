use std::io::ErrorKind;
use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::PgPool;

use crate::connectors::is_plausible_email;
use crate::db;
use crate::domain::{Company, Contact, ContactSource};
use crate::memory::{EntityRef, MemoryClient, MemoryError};
use crate::workflows::sync::SyncError;

const MAX_SKIPS_PER_FILE: usize = 100;
const MAX_CONSECUTIVE_BACKEND_FAILURES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipKind {
    Data,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowSkip {
    pub file: &'static str,
    pub line: u64,
    pub kind: SkipKind,
    pub reason: String,
}

#[derive(Debug, Default, Serialize)]
pub struct CsvSyncReport {
    pub contacts_upserted: u64,
    pub companies_upserted: u64,
    pub notes_memorized: u64,
    pub notes_already_imported: u64,
    pub skipped: Vec<RowSkip>,
    pub skipped_truncated: u64,
    pub fatal: Option<String>,
}

impl CsvSyncReport {
    fn skip(&mut self, file: &'static str, line: u64, kind: SkipKind, reason: String) {
        if self.skipped.len() < MAX_SKIPS_PER_FILE {
            self.skipped.push(RowSkip {
                file,
                line,
                kind,
                reason,
            });
        } else {
            self.skipped_truncated += 1;
        }
    }
}

#[derive(Debug, Deserialize)]
struct CompanyRow {
    domain: String,
    name: String,
    industry: String,
    employee_count: String,
    location: String,
}

#[derive(Debug, Deserialize)]
struct ContactRow {
    email: String,
    first_name: String,
    last_name: String,
    title: String,
    company_domain: String,
    linkedin_url: String,
}

#[derive(Debug, Deserialize)]
struct NoteRow {
    company_domain: String,
    note: String,
    noted_at: String,
}

const COMPANY_HEADERS: [&str; 5] = ["domain", "name", "industry", "employee_count", "location"];
const CONTACT_HEADERS: [&str; 6] = [
    "email",
    "first_name",
    "last_name",
    "title",
    "company_domain",
    "linkedin_url",
];
const NOTE_HEADERS: [&str; 3] = ["company_domain", "note", "noted_at"];

pub async fn sync_dir(
    pool: &PgPool,
    memory: &MemoryClient,
    dir: impl AsRef<Path>,
) -> Result<CsvSyncReport, SyncError> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        return Err(SyncError::Io {
            path: dir.display().to_string(),
            reason: "not a directory".into(),
        });
    }
    let mut report = CsvSyncReport::default();
    let companies = collect_rows::<CompanyRow>(dir, "companies.csv", &COMPANY_HEADERS, &mut report)?;
    for (line, row) in companies {
        if let Err(fatal) = upsert_company(pool, line, &row, &mut report).await {
            report.fatal = Some(fatal);
            return Ok(report);
        }
    }
    let contacts = collect_rows::<ContactRow>(dir, "contacts.csv", &CONTACT_HEADERS, &mut report)?;
    for (line, row) in contacts {
        if let Err(fatal) = upsert_contact(pool, line, &row, &mut report).await {
            report.fatal = Some(fatal);
            return Ok(report);
        }
    }
    let notes = collect_rows::<NoteRow>(dir, "notes.csv", &NOTE_HEADERS, &mut report)?;
    memorize_notes(pool, memory, notes, &mut report).await;
    Ok(report)
}

fn collect_rows<T: DeserializeOwned>(
    dir: &Path,
    file: &'static str,
    expected_headers: &[&str],
    report: &mut CsvSyncReport,
) -> Result<Vec<(u64, T)>, SyncError> {
    let path = dir.join(file);
    let mut reader = match ::csv::Reader::from_path(&path) {
        Ok(reader) => reader,
        Err(e) => {
            if let ::csv::ErrorKind::Io(io) = e.kind()
                && io.kind() == ErrorKind::NotFound
            {
                return Ok(Vec::new());
            }
            return Err(SyncError::Io {
                path: path.display().to_string(),
                reason: e.to_string(),
            });
        }
    };
    let found: Vec<String> = reader
        .headers()
        .map_err(|e| SyncError::Csv {
            file: file.to_string(),
            reason: e.to_string(),
        })?
        .iter()
        .map(str::to_string)
        .collect();
    if found != expected_headers {
        return Err(SyncError::Csv {
            file: file.to_string(),
            reason: format!("expected headers {expected_headers:?}, found {found:?}"),
        });
    }
    let mut rows = Vec::new();
    let mut iter = reader.deserialize::<T>();
    loop {
        let line = iter.reader().position().line();
        let Some(row) = iter.next() else { break };
        match row {
            Ok(row) => rows.push((line, row)),
            Err(e) => report.skip(file, line, SkipKind::Data, e.to_string()),
        }
    }
    Ok(rows)
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

async fn upsert_company(
    pool: &PgPool,
    line: u64,
    row: &CompanyRow,
    report: &mut CsvSyncReport,
) -> Result<(), String> {
    let file = "companies.csv";
    let employee_count = match non_empty(&row.employee_count) {
        None => None,
        Some(raw) => {
            let Ok(count) = raw.parse::<u32>() else {
                report.skip(
                    file,
                    line,
                    SkipKind::Data,
                    format!("employee_count {raw:?} is not a number"),
                );
                return Ok(());
            };
            Some(count)
        }
    };
    let mut company = Company::new(&row.domain);
    if company.domain.is_empty() {
        report.skip(file, line, SkipKind::Data, "empty domain".into());
        return Ok(());
    }
    company.name = non_empty(&row.name);
    company.industry = non_empty(&row.industry);
    company.employee_count = employee_count;
    company.location = non_empty(&row.location);
    match db::companies::upsert_import(pool, &company).await {
        Ok(_) => {
            report.companies_upserted += 1;
            Ok(())
        }
        Err(e) => Err(format!("companies.csv line {line}: {e}")),
    }
}

async fn upsert_contact(
    pool: &PgPool,
    line: u64,
    row: &ContactRow,
    report: &mut CsvSyncReport,
) -> Result<(), String> {
    let file = "contacts.csv";
    let email = row.email.trim();
    if !is_plausible_email(email) {
        report.skip(file, line, SkipKind::Data, format!("implausible email {email:?}"));
        return Ok(());
    }
    let mut contact = Contact::new(ContactSource::Csv);
    contact.email = Some(email.to_string());
    contact.first_name = non_empty(&row.first_name);
    contact.last_name = non_empty(&row.last_name);
    contact.title = non_empty(&row.title);
    contact.company_domain = non_empty(&row.company_domain);
    contact.linkedin_url = non_empty(&row.linkedin_url);
    match db::contacts::upsert_import(pool, &contact).await {
        Ok(_) => {
            report.contacts_upserted += 1;
            Ok(())
        }
        Err(e) => Err(format!("contacts.csv line {line}: {e}")),
    }
}

fn note_text(row: &NoteRow) -> Result<(String, String), String> {
    let domain = crate::domain::normalize_domain(&row.company_domain);
    if domain.is_empty() {
        return Err("empty company_domain".into());
    }
    let Some(note) = non_empty(&row.note) else {
        return Err("empty note".into());
    };
    match non_empty(&row.noted_at) {
        None => Ok((domain, note)),
        Some(raw_date) => match NaiveDate::parse_from_str(&raw_date, "%Y-%m-%d") {
            Ok(date) => Ok((domain, format!("[{date}] {note}"))),
            Err(_) => Err(format!("noted_at {raw_date:?} is not a YYYY-MM-DD date")),
        },
    }
}

async fn memorize_notes(
    pool: &PgPool,
    memory: &MemoryClient,
    notes: Vec<(u64, NoteRow)>,
    report: &mut CsvSyncReport,
) {
    let file = "notes.csv";
    let mut consecutive_backend_failures: u32 = 0;
    for (line, row) in notes {
        let (domain, text) = match note_text(&row) {
            Ok(parts) => parts,
            Err(reason) => {
                report.skip(file, line, SkipKind::Data, reason);
                continue;
            }
        };
        let ledger_key = format!("{domain}|{text}");
        let inserted =
            sqlx::query("INSERT INTO imported_notes (content_hash) VALUES (md5($1)) ON CONFLICT DO NOTHING")
                .bind(&ledger_key)
                .execute(pool)
                .await;
        match inserted {
            Ok(result) if result.rows_affected() == 0 => {
                report.notes_already_imported += 1;
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                report.fatal = Some(format!("notes.csv line {line}: {e}"));
                return;
            }
        }
        match memory.memorize(&EntityRef::company(&domain), &text, false).await {
            Ok(()) => {
                report.notes_memorized += 1;
                consecutive_backend_failures = 0;
            }
            Err(memory_error) => {
                let reason = match memory_error {
                    MemoryError::Backend(reason) => reason,
                    other => other.to_string(),
                };
                report.skip(file, line, SkipKind::Backend, reason);
                let _ = sqlx::query("DELETE FROM imported_notes WHERE content_hash = md5($1)")
                    .bind(&ledger_key)
                    .execute(pool)
                    .await;
                consecutive_backend_failures += 1;
                if consecutive_backend_failures >= MAX_CONSECUTIVE_BACKEND_FAILURES {
                    report.fatal = Some("memory service failed repeatedly; note import stopped".into());
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_row(domain: &str, note: &str, date: &str) -> NoteRow {
        NoteRow {
            company_domain: domain.into(),
            note: note.into(),
            noted_at: date.into(),
        }
    }

    #[test]
    fn non_empty_trims_and_rejects_whitespace() {
        assert_eq!(non_empty("  x  ").as_deref(), Some("x"));
        assert_eq!(non_empty("   "), None);
        assert_eq!(non_empty(""), None);
    }

    #[test]
    fn note_text_prefixes_valid_dates_and_rejects_junk() {
        let (domain, text) = note_text(&note_row("WWW.Acme.io", "Hiring reps", "2026-08-01")).unwrap();
        assert_eq!(domain, "acme.io");
        assert_eq!(text, "[2026-08-01] Hiring reps");
        let (_, undated) = note_text(&note_row("acme.io", "Hiring", "")).unwrap();
        assert_eq!(undated, "Hiring");
        assert!(note_text(&note_row("acme.io", "Hiring", "August 1st")).is_err());
        assert!(note_text(&note_row("", "Hiring", "2026-08-01")).is_err());
        assert!(note_text(&note_row("acme.io", "  ", "2026-08-01")).is_err());
    }

    #[test]
    fn skip_list_is_capped_and_counts_overflow() {
        let mut report = CsvSyncReport::default();
        for i in 0..(MAX_SKIPS_PER_FILE as u64 + 25) {
            report.skip("companies.csv", i, SkipKind::Data, "bad".into());
        }
        assert_eq!(report.skipped.len(), MAX_SKIPS_PER_FILE);
        assert_eq!(report.skipped_truncated, 25);
    }

    #[test]
    fn header_mismatch_is_a_csv_error() {
        let dir = std::env::temp_dir().join(format!("csv-hdr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("companies.csv"), "wrong,headers\n1,2\n").unwrap();
        let mut report = CsvSyncReport::default();
        let err =
            collect_rows::<CompanyRow>(&dir, "companies.csv", &COMPANY_HEADERS, &mut report).unwrap_err();
        assert!(matches!(err, SyncError::Csv { .. }));
    }

    #[test]
    fn line_numbers_survive_multiline_quoted_fields() {
        let dir = std::env::temp_dir().join(format!("csv-line-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("notes.csv"),
            "company_domain,note,noted_at\nacme.io,\"line one\nline two\",2026-08-01\nbeta.io,ok,2026-08-02\n",
        )
        .unwrap();
        let mut report = CsvSyncReport::default();
        let rows = collect_rows::<NoteRow>(&dir, "notes.csv", &NOTE_HEADERS, &mut report).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 2);
        assert_eq!(rows[1].0, 4);
    }

    #[test]
    fn missing_file_yields_no_rows_without_error() {
        let dir = std::env::temp_dir().join(format!("csv-miss-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut report = CsvSyncReport::default();
        let rows = collect_rows::<NoteRow>(&dir, "notes.csv", &NOTE_HEADERS, &mut report).unwrap();
        assert!(rows.is_empty());
    }
}
