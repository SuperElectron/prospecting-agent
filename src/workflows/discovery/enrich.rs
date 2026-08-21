use serde::Serialize;
use sqlx::PgPool;

use crate::clients::apollo::ApolloClient;
use crate::clients::company_from_organization;
use crate::db;
use crate::domain::ContactStatus;
use crate::memory::{EntityRef, MemoryClient};
use crate::workflows::sync::{SyncError, ingest_person};

#[derive(Debug, Default, Serialize)]
pub struct EnrichmentReport {
    pub enriched: u64,
    pub no_match: u64,
    pub skipped: u64,
    pub failed: u64,
}

pub async fn enrich_contacts(
    pool: &PgPool,
    memory: &MemoryClient,
    apollo: &ApolloClient,
    limit: i64,
) -> Result<EnrichmentReport, SyncError> {
    let mut report = EnrichmentReport::default();
    let contacts = db::contacts::list_by_status(pool, ContactStatus::New, limit).await?;
    for contact in contacts {
        let Some(email) = contact.email.as_deref() else {
            report.skipped += 1;
            continue;
        };
        match apollo.match_person(email).await {
            Ok(Some(person)) => {
                ingest_person(pool, memory, &person).await?;
                db::contacts::set_status(pool, contact.id, ContactStatus::Enriched).await?;
                report.enriched += 1;
            }
            Ok(None) => {
                report.no_match += 1;
            }
            Err(e) => {
                tracing::warn!(email, error = %e, "apollo person match failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

pub async fn enrich_companies(
    pool: &PgPool,
    memory: &MemoryClient,
    apollo: &ApolloClient,
    limit: i64,
) -> Result<EnrichmentReport, SyncError> {
    let mut report = EnrichmentReport::default();
    let companies = db::companies::list_unenriched(pool, limit).await?;
    for company in companies {
        match apollo.enrich_organization(&company.domain).await {
            Ok(Some(organization)) => {
                let Some(enriched) = company_from_organization(&organization) else {
                    report.no_match += 1;
                    continue;
                };
                db::companies::upsert(pool, &enriched).await?;
                let line = company_line(&enriched);
                if let Err(e) = memory
                    .memorize(&EntityRef::company(&enriched.domain), &line, false)
                    .await
                {
                    tracing::warn!(domain = enriched.domain, error = %e, "company memory line failed");
                }
                report.enriched += 1;
            }
            Ok(None) => {
                report.no_match += 1;
            }
            Err(e) => {
                tracing::warn!(domain = company.domain, error = %e, "apollo org enrich failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

fn company_line(company: &crate::domain::Company) -> String {
    let mut parts = vec![format!("[ENRICHED apollo] {}", company.domain)];
    if let Some(industry) = &company.industry {
        parts.push(format!("industry: {industry}"));
    }
    if let Some(count) = company.employee_count {
        parts.push(format!("{count} employees"));
    }
    if let Some(location) = &company.location {
        parts.push(location.clone());
    }
    parts.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Company;

    #[test]
    fn company_line_reads_well_with_partial_fields() {
        let mut company = Company::new("acme.io");
        company.industry = Some("Software".into());
        company.employee_count = Some(120);
        assert_eq!(
            company_line(&company),
            "[ENRICHED apollo] acme.io | industry: Software | 120 employees"
        );
        assert_eq!(
            company_line(&Company::new("bare.io")),
            "[ENRICHED apollo] bare.io"
        );
    }
}
