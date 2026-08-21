use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Company {
    pub id: Uuid,
    pub domain: String,
    pub name: Option<String>,
    pub industry: Option<String>,
    pub employee_count: Option<u32>,
    pub location: Option<String>,
    pub linkedin_url: Option<String>,
    pub crm_id: Option<String>,
    pub hiring_velocity: Option<HiringVelocity>,
    pub icp_fit_score: Option<u8>,
    pub summary: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Company {
    pub fn new(domain: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            domain: normalize_domain(&domain.into()),
            name: None,
            industry: None,
            employee_count: None,
            location: None,
            linkedin_url: None,
            crm_id: None,
            hiring_velocity: None,
            icp_fit_score: None,
            summary: None,
            created_at: now,
            updated_at: now,
        }
    }
}

pub fn normalize_domain(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    let stripped = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let stripped = stripped.strip_prefix("www.").unwrap_or(stripped);
    stripped.split('/').next().unwrap_or(stripped).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiringVelocity {
    Contracting,
    Stable,
    ModerateGrowth,
    RapidGrowth,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_domain_strips_scheme_www_and_path() {
        assert_eq!(normalize_domain("https://www.Acme.io/about"), "acme.io");
        assert_eq!(normalize_domain("http://acme.io"), "acme.io");
        assert_eq!(normalize_domain("  ACME.IO  "), "acme.io");
        assert_eq!(normalize_domain("acme.io"), "acme.io");
    }

    #[test]
    fn new_company_normalizes_its_domain() {
        let c = Company::new("https://www.Example.com/pricing");
        assert_eq!(c.domain, "example.com");
    }

    #[test]
    fn serde_roundtrip_preserves_company() {
        let mut c = Company::new("acme.io");
        c.hiring_velocity = Some(HiringVelocity::RapidGrowth);
        let json = serde_json::to_string(&c).unwrap();
        let back: Company = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
