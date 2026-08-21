use serde::Serialize;
use sqlx::PgPool;

use crate::clients::apollo::{ApolloClient, PeopleSearchParams};
use crate::clients::usable_email;
use crate::config::{DiscoveryBudget, IcpCriteria};
use crate::db;
use crate::domain::Seniority;
use crate::memory::MemoryClient;
use crate::workflows::discovery::DiscoveryError;
use crate::workflows::sync::ingest_person;

#[derive(Debug, Default, Serialize)]
pub struct DiscoveryReport {
    pub discovered: u64,
    pub already_known: u64,
    pub no_match: u64,
    pub no_email: u64,
    pub search_failed: u64,
    pub match_failed: u64,
    pub credits_spent: u64,
}

impl DiscoveryReport {
    pub(crate) fn absorb(&mut self, other: &DiscoveryReport) {
        self.discovered += other.discovered;
        self.already_known += other.already_known;
        self.no_match += other.no_match;
        self.no_email += other.no_email;
        self.search_failed += other.search_failed;
        self.match_failed += other.match_failed;
        self.credits_spent += other.credits_spent;
    }
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
) -> Result<DiscoveryReport, DiscoveryError> {
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
            report.search_failed += 1;
            return Ok(report);
        }
    };
    let mut spent_this_account: u8 = 0;
    for candidate in candidates {
        if spent_this_account >= budget.contacts_per_account || *credits_left == 0 {
            break;
        }
        if candidate.id.is_empty() {
            report.match_failed += 1;
            continue;
        }
        match db::contacts::by_crm_id(pool, &candidate.id).await {
            Ok(Some(_)) => {
                report.already_known += 1;
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(domain, error = %e, "crm id lookup failed");
                report.match_failed += 1;
                continue;
            }
        }
        *credits_left -= 1;
        spent_this_account += 1;
        report.credits_spent += 1;
        match apollo.match_person_by_id(&candidate.id).await {
            Ok(Some(person)) => {
                if let Err(e) = land_person(pool, memory, &candidate.id, &person, &mut report).await {
                    tracing::warn!(domain, error = %e, "landing a matched person failed");
                    report.match_failed += 1;
                }
            }
            Ok(None) => {
                report.no_match += 1;
            }
            Err(e) => {
                tracing::warn!(domain, candidate = candidate.id, error = %e, "apollo match failed");
                report.match_failed += 1;
            }
        }
    }
    Ok(report)
}

async fn land_person(
    pool: &PgPool,
    memory: &MemoryClient,
    candidate_id: &str,
    person: &crate::clients::ApolloPerson,
    report: &mut DiscoveryReport,
) -> Result<(), DiscoveryError> {
    let Some(email) = usable_email(person.email.as_deref()) else {
        report.no_email += 1;
        return Ok(());
    };
    if let Some(existing) = db::contacts::by_email(pool, email).await? {
        if existing.crm_id.is_none() {
            db::contacts::set_crm_id(pool, existing.id, candidate_id).await?;
        }
        report.already_known += 1;
        return Ok(());
    }
    let contact_id = ingest_person(pool, memory, person).await?;
    db::contacts::advance_status(
        pool,
        contact_id,
        crate::domain::ContactStatus::New,
        crate::domain::ContactStatus::Enriched,
    )
    .await?;
    report.discovered += 1;
    Ok(())
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
