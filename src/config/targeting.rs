use serde::{Deserialize, Serialize};

use crate::domain::Seniority;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TargetingError {
    #[error("score weights must sum to 100, got {sum}")]
    WeightsNotHundred { sum: u16 },
    #[error("employee range is inverted: {min} > {max}")]
    InvertedEmployeeRange { min: u32, max: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IcpCriteria {
    pub industries: Vec<String>,
    pub employee_range: (u32, u32),
    pub target_titles: Vec<String>,
    pub min_seniority: Seniority,
    pub disqualified_domains: Vec<String>,
    pub weights: ScoreWeights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreWeights {
    pub icp_fit: u8,
    pub buying_signals: u8,
    pub engagement: u8,
    pub champion_potential: u8,
}

impl ScoreWeights {
    fn sum(self) -> u16 {
        u16::from(self.icp_fit)
            + u16::from(self.buying_signals)
            + u16::from(self.engagement)
            + u16::from(self.champion_potential)
    }

    pub fn is_valid(&self) -> bool {
        self.sum() == 100
    }
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            icp_fit: 40,
            buying_signals: 30,
            engagement: 20,
            champion_potential: 10,
        }
    }
}

impl IcpCriteria {
    pub fn validate(&self) -> Result<(), TargetingError> {
        if !self.weights.is_valid() {
            return Err(TargetingError::WeightsNotHundred {
                sum: self.weights.sum(),
            });
        }
        if self.employee_range.0 > self.employee_range.1 {
            return Err(TargetingError::InvertedEmployeeRange {
                min: self.employee_range.0,
                max: self.employee_range.1,
            });
        }
        Ok(())
    }

    pub fn company_size_fits(&self, employee_count: u32) -> bool {
        employee_count >= self.employee_range.0 && employee_count <= self.employee_range.1
    }

    pub fn is_disqualified(&self, domain: &str) -> bool {
        self.disqualified_domains.iter().any(|d| d == domain)
    }

    pub fn seniority_fits(&self, seniority: Seniority) -> bool {
        seniority >= self.min_seniority
    }
}

impl Default for IcpCriteria {
    fn default() -> Self {
        Self {
            industries: vec!["B2B SaaS".into(), "Technology".into()],
            employee_range: (20, 2000),
            target_titles: vec![
                "VP Sales".into(),
                "Head of Sales".into(),
                "CRO".into(),
                "Revenue Operations".into(),
            ],
            min_seniority: Seniority::Director,
            disqualified_domains: Vec::new(),
            weights: ScoreWeights::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_weights_sum_to_one_hundred() {
        assert!(ScoreWeights::default().is_valid());
    }

    #[test]
    fn skewed_weights_are_invalid() {
        let w = ScoreWeights {
            icp_fit: 90,
            buying_signals: 30,
            engagement: 20,
            champion_potential: 10,
        };
        assert!(!w.is_valid());
    }

    #[test]
    fn validate_enforces_weights_and_range() {
        assert!(IcpCriteria::default().validate().is_ok());
        let skewed = IcpCriteria {
            weights: ScoreWeights {
                icp_fit: 90,
                ..ScoreWeights::default()
            },
            ..IcpCriteria::default()
        };
        assert_eq!(
            skewed.validate(),
            Err(TargetingError::WeightsNotHundred { sum: 150 })
        );
        let inverted = IcpCriteria {
            employee_range: (500, 20),
            ..IcpCriteria::default()
        };
        assert_eq!(
            inverted.validate(),
            Err(TargetingError::InvertedEmployeeRange { min: 500, max: 20 })
        );
    }

    #[test]
    fn company_size_bounds_are_inclusive() {
        let icp = IcpCriteria::default();
        assert!(icp.company_size_fits(20));
        assert!(icp.company_size_fits(2000));
        assert!(!icp.company_size_fits(19));
        assert!(!icp.company_size_fits(2001));
    }

    #[test]
    fn seniority_gate_uses_ladder_ordering() {
        let icp = IcpCriteria::default();
        assert!(icp.seniority_fits(Seniority::Vp));
        assert!(icp.seniority_fits(Seniority::Director));
        assert!(!icp.seniority_fits(Seniority::Manager));
    }

    #[test]
    fn disqualified_domains_block() {
        let mut icp = IcpCriteria::default();
        icp.disqualified_domains.push("rival.io".into());
        assert!(icp.is_disqualified("rival.io"));
        assert!(!icp.is_disqualified("acme.io"));
    }
}
