use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMode {
    pub name: String,
    pub max_emails_per_day: u16,
    pub max_linkedin_per_day: u16,
    pub min_score_to_contact: u8,
    pub auto_send: bool,
    pub cadence: String,
}

impl AgentMode {
    pub fn conservative() -> Self {
        Self {
            name: "conservative".into(),
            max_emails_per_day: 10,
            max_linkedin_per_day: 5,
            min_score_to_contact: 70,
            auto_send: false,
            cadence: "gentle".into(),
        }
    }

    pub fn standard() -> Self {
        Self {
            name: "standard".into(),
            max_emails_per_day: 50,
            max_linkedin_per_day: 15,
            min_score_to_contact: 50,
            auto_send: true,
            cadence: "standard".into(),
        }
    }

    pub fn aggressive() -> Self {
        Self {
            name: "aggressive".into(),
            max_emails_per_day: 150,
            max_linkedin_per_day: 25,
            min_score_to_contact: 35,
            auto_send: true,
            cadence: "standard".into(),
        }
    }

    pub fn allows_contact(&self, score: u8) -> bool {
        score >= self.min_score_to_contact
    }
}

pub fn registry() -> Vec<AgentMode> {
    vec![
        AgentMode::conservative(),
        AgentMode::standard(),
        AgentMode::aggressive(),
    ]
}

pub fn by_name(name: &str) -> Option<AgentMode> {
    registry().into_iter().find(|m| m.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::cadence;

    #[test]
    fn presets_escalate_volume_and_lower_the_bar() {
        let c = AgentMode::conservative();
        let s = AgentMode::standard();
        let a = AgentMode::aggressive();
        assert!(c.max_emails_per_day < s.max_emails_per_day);
        assert!(s.max_emails_per_day < a.max_emails_per_day);
        assert!(c.min_score_to_contact > s.min_score_to_contact);
        assert!(s.min_score_to_contact > a.min_score_to_contact);
    }

    #[test]
    fn conservative_never_auto_sends() {
        assert!(!AgentMode::conservative().auto_send);
    }

    #[test]
    fn score_gate_is_inclusive() {
        let m = AgentMode::standard();
        assert!(m.allows_contact(50));
        assert!(!m.allows_contact(49));
    }

    #[test]
    fn every_mode_references_a_real_cadence() {
        for mode in registry() {
            assert!(cadence::by_name(&mode.cadence).is_some(), "mode {}", mode.name);
        }
    }

    #[test]
    fn registry_lookup_by_name() {
        assert_eq!(by_name("aggressive").unwrap().max_emails_per_day, 150);
        assert!(by_name("nope").is_none());
    }
}
