use crate::clients::apollo::{ApolloOrganization, ApolloPerson};
use crate::domain;
use crate::domain::{Company, Contact, ContactSource, Seniority};

pub fn seniority_from_apollo(raw: &str) -> Option<Seniority> {
    match raw.to_lowercase().as_str() {
        "c_suite" | "c-suite" | "csuite" => Some(Seniority::CSuite),
        "founder" | "owner" => Some(Seniority::Founder),
        "vp" | "head" | "partner" => Some(Seniority::Vp),
        "director" => Some(Seniority::Director),
        "manager" => Some(Seniority::Manager),
        "senior" | "entry" | "intern" | "individual" => Some(Seniority::Individual),
        unknown => {
            tracing::debug!(seniority = unknown, "unmapped apollo seniority value");
            None
        }
    }
}

pub fn usable_email(raw: Option<&str>) -> Option<&str> {
    raw.filter(|e| !e.is_empty() && !e.contains("not_unlocked"))
}

pub fn contact_from_person(person: &ApolloPerson) -> Contact {
    let mut contact = Contact::new(ContactSource::Apollo);
    contact.email = usable_email(person.email.as_deref()).map(str::to_string);
    contact.first_name.clone_from(&person.first_name);
    contact.last_name.clone_from(&person.last_name);
    contact.title.clone_from(&person.title);
    contact.seniority = person.seniority.as_deref().and_then(seniority_from_apollo);
    contact.linkedin_url.clone_from(&person.linkedin_url);
    contact.crm_id = Some(person.id.clone()).filter(|id| !id.is_empty());
    contact.company_domain = person.organization.as_ref().and_then(org_domain);
    contact
}

pub fn company_from_organization(org: &ApolloOrganization) -> Option<Company> {
    let domain = org_domain(org)?;
    let mut company = Company::new(domain);
    company.name.clone_from(&org.name);
    company.industry.clone_from(&org.industry);
    company.employee_count = org.estimated_num_employees;
    company.location = location(org);
    company.linkedin_url.clone_from(&org.linkedin_url);
    company.crm_id = Some(org.id.clone()).filter(|id| !id.is_empty());
    company.summary.clone_from(&org.short_description);
    Some(company)
}

fn org_domain(org: &ApolloOrganization) -> Option<String> {
    org.primary_domain
        .clone()
        .filter(|d| !d.is_empty())
        .or_else(|| org.website_url.clone().filter(|u| !u.is_empty()))
        .map(|d| domain::normalize_domain(&d))
        .filter(|d| !d.is_empty())
}

fn location(org: &ApolloOrganization) -> Option<String> {
    let parts: Vec<&str> = [org.city.as_deref(), org.state.as_deref(), org.country.as_deref()]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org() -> ApolloOrganization {
        ApolloOrganization {
            id: "org-1".into(),
            name: Some("Acme".into()),
            website_url: Some("https://www.Acme.io/home".into()),
            primary_domain: Some("acme.io".into()),
            industry: Some("Software".into()),
            estimated_num_employees: Some(120),
            city: Some("Austin".into()),
            state: Some("Texas".into()),
            country: Some("United States".into()),
            ..ApolloOrganization::default()
        }
    }

    #[test]
    fn person_maps_to_contact_with_normalized_company_domain() {
        let person = ApolloPerson {
            id: "p-1".into(),
            first_name: Some("Jane".into()),
            title: Some("VP Sales".into()),
            email: Some("jane@acme.io".into()),
            seniority: Some("vp".into()),
            organization: Some(ApolloOrganization {
                primary_domain: Some("WWW.Acme.IO".into()),
                ..org()
            }),
            ..ApolloPerson::default()
        };
        let contact = contact_from_person(&person);
        assert_eq!(contact.email.as_deref(), Some("jane@acme.io"));
        assert_eq!(contact.seniority, Some(Seniority::Vp));
        assert_eq!(contact.company_domain.as_deref(), Some("acme.io"));
        assert_eq!(contact.crm_id.as_deref(), Some("p-1"));
        assert_eq!(contact.source, ContactSource::Apollo);
    }

    #[test]
    fn locked_placeholder_email_is_dropped() {
        let person = ApolloPerson {
            email: Some("email_not_unlocked@domain.com".into()),
            ..ApolloPerson::default()
        };
        assert_eq!(contact_from_person(&person).email, None);
    }

    #[test]
    fn organization_maps_to_company_with_joined_location() {
        let company = company_from_organization(&org()).unwrap();
        assert_eq!(company.domain, "acme.io");
        assert_eq!(company.employee_count, Some(120));
        assert_eq!(company.location.as_deref(), Some("Austin, Texas, United States"));
    }

    #[test]
    fn organization_without_any_domain_maps_to_none() {
        let bare = ApolloOrganization {
            primary_domain: None,
            website_url: None,
            ..org()
        };
        assert!(company_from_organization(&bare).is_none());
    }

    #[test]
    fn website_url_fallback_is_normalized() {
        let fallback = ApolloOrganization {
            primary_domain: Some(String::new()),
            website_url: Some("https://www.Beta-Corp.com/about".into()),
            ..org()
        };
        let company = company_from_organization(&fallback).unwrap();
        assert_eq!(company.domain, "beta-corp.com");
    }

    #[test]
    fn obfuscated_free_tier_person_maps_to_sparse_contact() {
        let raw = r#"{"id":"p-1","first_name":"Jane","email":null,"seniority":null,"organization":null,"departments":null}"#;
        let person: ApolloPerson = serde_json::from_str(raw).unwrap();
        let contact = contact_from_person(&person);
        assert_eq!(contact.email, None);
        assert_eq!(contact.seniority, None);
        assert_eq!(contact.company_domain, None);
        assert_eq!(contact.crm_id.as_deref(), Some("p-1"));
    }

    #[test]
    fn seniority_ladder_covers_apollo_vocabulary() {
        assert_eq!(seniority_from_apollo("c_suite"), Some(Seniority::CSuite));
        assert_eq!(seniority_from_apollo("Founder"), Some(Seniority::Founder));
        assert_eq!(seniority_from_apollo("head"), Some(Seniority::Vp));
        assert_eq!(seniority_from_apollo("director"), Some(Seniority::Director));
        assert_eq!(seniority_from_apollo("partner"), Some(Seniority::Vp));
        assert_eq!(seniority_from_apollo("senior"), Some(Seniority::Individual));
        assert_eq!(seniority_from_apollo("mystery"), None);
    }
}
