use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountStage {
    Prospecting,
    Engaged,
    Opportunity,
    MultiThreaded,
    Customer,
    Dormant,
}

impl AccountStage {
    pub fn is_advanced(self) -> bool {
        matches!(self, Self::Engaged | Self::Opportunity | Self::MultiThreaded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountHealth {
    Healthy,
    Watch,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountStrategy {
    pub domain: String,
    pub stage: AccountStage,
    pub health: AccountHealth,
    pub coordination_flags: Vec<String>,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}

pub const FLAG_NEGATIVE_EVENT: &str = "negative_company_event";
pub const FLAG_CARPET_BOMB: &str = "carpet_bomb_risk";
pub const FLAG_NEW_CONTACT_ADVANCED: &str = "new_contact_at_advanced_account";
pub const FLAG_CONVERTED: &str = "account_converted";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_stages_are_the_middle_of_the_funnel() {
        assert!(AccountStage::Engaged.is_advanced());
        assert!(AccountStage::MultiThreaded.is_advanced());
        assert!(!AccountStage::Prospecting.is_advanced());
        assert!(!AccountStage::Customer.is_advanced());
    }

    #[test]
    fn serde_uses_snake_case_wire_names() {
        assert_eq!(
            serde_json::to_value(AccountStage::MultiThreaded).unwrap(),
            "multi_threaded"
        );
        assert_eq!(serde_json::to_value(AccountHealth::Watch).unwrap(), "watch");
    }
}
