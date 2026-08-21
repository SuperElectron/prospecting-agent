use serde::Serialize;
use sqlx::PgPool;

use crate::clients::apollo::{ApolloClient, PeopleSearchParams};
use crate::config::IcpCriteria;
use crate::db;
use crate::domain::Seniority;
use crate::memory::MemoryClient;
use crate::workflows::sync::{SyncError, ingest_person};

#[derive(Debug, Clone, Copy)]
pub struct DiscoveryBudget {
    pub contacts_per_account: u8,
    pub max_credits_per_run: u16,
}

impl Default for DiscoveryBudget {
    fn default() -> Self {
        Self {
            contacts_per_account: 3,
            max_credits_per_run: 15,
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct DiscoveryReport {
    pub discovered: u64,
    pub already_known: u64,
    pub no_email: u64,
    pub failed: u64,
    pub credits_spent: u64,
}

fn seniority_filters(minimum: Seniority) -> Vec<String> {
    let ladder = [
        (Seniority::Founder, "founder"),
        (Seniority::CSuite, "c_suite"),
        (Seniority::Vp, "vp"),
        (Seniority::Director, "director"),
        (Seniority::Manager, "manager"),
        (Seniority::Individual, "senior"),
    ];
    ladder
        .into_iter()
        .filter(|(level, _)| *level >= minimum)
        .map(|(_, name)| name.to_string())
        .collect()
}

pub async fn discover_contacts(
    pool: &PgPool,
    memory: &MemoryClient,
    apollo: &ApolloClient,
    icp: &IcpCriteria,
    domain: &str,
    budget: &DiscoveryBudget,
    credits_left: &mut u16,
) -> Result<DiscoveryReport, SyncError> {
    let mut report = DiscoveryReport::default();
    let params = PeopleSearchParams {
        organization_domains: vec![domain.to_string()],
        person_titles: icp.target_titles.clone(),
        person_seniorities: seniority_filters(icp.min_seniority),
        per_page: Some(budget.contacts_per_account.saturating_mul(3).min(25)),
        ..PeopleSearchParams::default()
    };
    let candidates = match apollo.search_people(&params).await {
        Ok(response) => response.people,
        Err(e) => {
            tracing::warn!(domain, error = %e, "apollo people search failed");
            report.failed += 1;
            return Ok(report);
        }
    };
    for candidate in candidates {
        if report.discovered >= u64::from(budget.contacts_per_account) {
            break;
        }
        if *credits_left == 0 {
            break;
        }
        if candidate.id.is_empty() {
            report.failed += 1;
            continue;
        }
        if db::contacts::by_crm_id(pool, &candidate.id).await?.is_some() {
            report.already_known += 1;
            continue;
        }
        *credits_left -= 1;
        report.credits_spent += 1;
        match apollo.match_person_by_id(&candidate.id).await {
            Ok(Some(person)) => {
                let email = person.email.as_deref().unwrap_or_default();
                if email.is_empty() {
                    report.no_email += 1;
                    continue;
                }
                if db::contacts::by_email(pool, email).await?.is_some() {
                    report.already_known += 1;
                    continue;
                }
                let contact_id = ingest_person(pool, memory, &person).await?;
                db::contacts::set_status(pool, contact_id, crate::domain::ContactStatus::Enriched).await?;
                report.discovered += 1;
            }
            Ok(None) => {
                report.no_email += 1;
            }
            Err(e) => {
                tracing::warn!(domain, candidate = candidate.id, error = %e, "apollo match failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

pub async fn source_contacts(
    pool: &PgPool,
    memory: &MemoryClient,
    apollo: &ApolloClient,
    icp: &IcpCriteria,
    domains: &[String],
    budget: &DiscoveryBudget,
) -> Result<DiscoveryReport, SyncError> {
    let mut total = DiscoveryReport::default();
    let mut credits_left = budget.max_credits_per_run;
    for domain in domains {
        if credits_left == 0 {
            break;
        }
        let report = discover_contacts(pool, memory, apollo, icp, domain, budget, &mut credits_left).await?;
        total.discovered += report.discovered;
        total.already_known += report.already_known;
        total.no_email += report.no_email;
        total.failed += report.failed;
        total.credits_spent += report.credits_spent;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seniority_filters_take_the_minimum_and_above() {
        let filters = seniority_filters(Seniority::Director);
        assert_eq!(filters, vec!["founder", "c_suite", "vp", "director"]);
        let all = seniority_filters(Seniority::Individual);
        assert_eq!(all.len(), 6);
    }
}
