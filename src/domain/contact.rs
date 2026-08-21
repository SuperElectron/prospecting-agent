use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub id: Uuid,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub title: Option<String>,
    pub seniority: Option<Seniority>,
    pub linkedin_url: Option<String>,
    pub company_domain: Option<String>,
    pub crm_id: Option<String>,
    pub source: ContactSource,
    pub score: Option<u8>,
    pub status: ContactStatus,
    pub assigned_sender: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Contact {
    pub fn new(source: ContactSource) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            email: None,
            first_name: None,
            last_name: None,
            title: None,
            seniority: None,
            linkedin_url: None,
            company_domain: None,
            crm_id: None,
            source,
            score: None,
            status: ContactStatus::New,
            assigned_sender: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn full_name(&self) -> Option<String> {
        match (&self.first_name, &self.last_name) {
            (Some(f), Some(l)) => Some(format!("{f} {l}")),
            (Some(f), None) => Some(f.clone()),
            (None, Some(l)) => Some(l.clone()),
            (None, None) => None,
        }
    }

    pub fn is_contactable(&self) -> bool {
        !matches!(self.status, ContactStatus::OptedOut | ContactStatus::Disqualified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactStatus {
    New,
    NoMatch,
    Enriched,
    Scored,
    InSequence,
    Replied,
    MeetingBooked,
    OptedOut,
    Disqualified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactSource {
    Csv,
    Hubspot,
    Apollo,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seniority {
    Individual,
    Manager,
    Director,
    Vp,
    CSuite,
    Founder,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_contact_starts_fresh_and_contactable() {
        let c = Contact::new(ContactSource::Csv);
        assert_eq!(c.status, ContactStatus::New);
        assert!(c.is_contactable());
        assert!(c.score.is_none());
    }

    #[test]
    fn opted_out_and_disqualified_are_not_contactable() {
        let mut c = Contact::new(ContactSource::Apollo);
        c.status = ContactStatus::OptedOut;
        assert!(!c.is_contactable());
        c.status = ContactStatus::Disqualified;
        assert!(!c.is_contactable());
    }

    #[test]
    fn full_name_handles_partial_names() {
        let mut c = Contact::new(ContactSource::Manual);
        assert_eq!(c.full_name(), None);
        c.first_name = Some("Ada".into());
        assert_eq!(c.full_name().as_deref(), Some("Ada"));
        c.last_name = Some("Lovelace".into());
        assert_eq!(c.full_name().as_deref(), Some("Ada Lovelace"));
    }

    #[test]
    fn seniority_orders_up_the_ladder() {
        assert!(Seniority::CSuite > Seniority::Vp);
        assert!(Seniority::Vp > Seniority::Director);
        assert!(Seniority::Director > Seniority::Manager);
    }

    #[test]
    fn serde_roundtrip_preserves_contact() {
        let c = Contact::new(ContactSource::Hubspot);
        let json = serde_json::to_string(&c).unwrap();
        let back: Contact = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
