use serde::Serialize;
use sqlx::PgPool;

use crate::clients::apollo::ApolloClient;
use crate::clients::company_from_organization;
use crate::db;
use crate::domain::ContactStatus;
use crate::memory::{EntityRef, MemoryClient};
use crate::workflows::discovery::DiscoveryError;
use crate::workflows::sync::ingest_person;

#[derive(Debug, Default, Serialize)]
pub struct EnrichmentReport {
    pub enriched: u64,
    pub no_match: u64,
    pub skipped: u64,
    pub failed: u64,
    pub credits_spent: u64,
}

pub async fn enrich_contacts(
    pool: &PgPool,
    memory: &MemoryClient,
    apollo: &ApolloClient,
    limit: i64,
    credits_left: &mut u16,
) -> Result<EnrichmentReport, DiscoveryError> {
    let mut report = EnrichmentReport::default();
    let contacts = db::contacts::list_by_status(pool, ContactStatus::New, limit).await?;
    for contact in contacts {
        if *credits_left == 0 {
            break;
        }
        let Some(email) = contact.email.as_deref() else {
            db::contacts::advance_status(pool, contact.id, ContactStatus::New, ContactStatus::NoMatch)
                .await?;
            report.skipped += 1;
            continue;
        };
        *credits_left -= 1;
        report.credits_spent += 1;
        match apollo.match_person(email).await {
            Ok(Some(person)) => {
                let enriched_id = ingest_person(pool, memory, &person).await?;
                if enriched_id != contact.id {
                    tracing::warn!(
                        queried = %contact.id,
                        resolved = %enriched_id,
                        "enrichment resolved to a different contact"
                    );
                    db::contacts::advance_status(
                        pool,
                        contact.id,
                        ContactStatus::New,
                        ContactStatus::NoMatch,
                    )
                    .await?;
                }
                db::contacts::advance_status(pool, enriched_id, ContactStatus::New, ContactStatus::Enriched)
                    .await?;
                report.enriched += 1;
            }
            Ok(None) => {
                db::contacts::advance_status(pool, contact.id, ContactStatus::New, ContactStatus::NoMatch)
                    .await?;
                report.no_match += 1;
            }
            Err(e) => {
                tracing::warn!(contact = %contact.id, error = %e, "apollo person match failed");
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
    credits_left: &mut u16,
) -> Result<EnrichmentReport, DiscoveryError> {
    let mut report = EnrichmentReport::default();
    let companies = db::companies::list_unenriched(pool, limit).await?;
    for company in companies {
        if *credits_left == 0 {
            break;
        }
        *credits_left -= 1;
        report.credits_spent += 1;
        match apollo.enrich_organization(&company.domain).await {
            Ok(Some(organization)) => {
                db::companies::mark_enrichment_attempted(pool, &company.domain).await?;
                let Some(mut enriched) = company_from_organization(&organization) else {
                    report.no_match += 1;
                    continue;
                };
                if enriched.domain != company.domain {
                    tracing::warn!(
                        queried = company.domain,
                        canonical = enriched.domain,
                        "apollo reports a different canonical domain; keeping the queried row"
                    );
                    enriched.domain.clone_from(&company.domain);
                }
                db::companies::upsert_enrichment(pool, &enriched).await?;
                let line = company_line(&enriched);
                crate::workflows::util::best_effort_memorize(
                    memory,
                    &EntityRef::company(&enriched.domain),
                    &line,
                )
                .await;
                report.enriched += 1;
            }
            Ok(None) => {
                db::companies::mark_enrichment_attempted(pool, &company.domain).await?;
                report.no_match += 1;
            }
            Err(e) => {
                db::companies::mark_enrichment_attempted(pool, &company.domain).await?;
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
