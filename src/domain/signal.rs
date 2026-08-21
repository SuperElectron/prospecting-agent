use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub id: Uuid,
    pub company_domain: String,
    pub kind: SignalKind,
    pub strength: SignalStrength,
    pub summary: String,
    pub source_url: Option<String>,
    pub detected_at: DateTime<Utc>,
}

impl Signal {
    pub fn new(
        company_domain: impl Into<String>,
        kind: SignalKind,
        strength: SignalStrength,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_domain: company_domain.into(),
            kind,
            strength,
            summary: summary.into(),
            source_url: None,
            detected_at: Utc::now(),
        }
    }

    pub fn score_delta(&self) -> u8 {
        match self.strength {
            SignalStrength::Strong => 30,
            SignalStrength::Moderate => 15,
            SignalStrength::Weak => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Funding,
    Hiring,
    LeadershipChange,
    Expansion,
    ContentMention,
    TechAdoption,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalStrength {
    Weak,
    Moderate,
    Strong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_signals_carry_bigger_score_deltas() {
        let strong = Signal::new("acme.io", SignalKind::Funding, SignalStrength::Strong, "raised B");
        let moderate = Signal::new(
            "acme.io",
            SignalKind::Hiring,
            SignalStrength::Moderate,
            "hiring ops",
        );
        let weak = Signal::new("acme.io", SignalKind::Other, SignalStrength::Weak, "minor");
        assert!(strong.score_delta() > moderate.score_delta());
        assert!(moderate.score_delta() > weak.score_delta());
    }

    #[test]
    fn strength_ordering_matches_semantics() {
        assert!(SignalStrength::Strong > SignalStrength::Moderate);
        assert!(SignalStrength::Moderate > SignalStrength::Weak);
    }

    #[test]
    fn serde_roundtrip_preserves_signal() {
        let s = Signal::new(
            "acme.io",
            SignalKind::LeadershipChange,
            SignalStrength::Strong,
            "new CRO",
        );
        let json = serde_json::to_string(&s).unwrap();
        let back: Signal = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
