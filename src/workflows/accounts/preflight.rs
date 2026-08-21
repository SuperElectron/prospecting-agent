use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

use crate::db;
use crate::domain::{
    AccountHealth, AccountStage, AccountStrategy, Contact, ContactStatus, FLAG_CARPET_BOMB, FLAG_CONVERTED,
    FLAG_NEGATIVE_EVENT, FLAG_NEW_CONTACT_ADVANCED,
};
use crate::workflows::accounts::AccountError;

#[derive(Debug, Clone, Copy)]
pub struct PreflightConfig {
    pub carpet_bomb_window_days: i32,
    pub max_contacts_per_window: i64,
    pub negative_event_delay_days: i64,
}

impl Default for PreflightConfig {
    fn default() -> Self {
        Self {
            carpet_bomb_window_days: 7,
            max_contacts_per_window: 2,
            negative_event_delay_days: 21,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightDecision {
    Proceed,
    Modify { cadence: String, max_emails: u8 },
    Delay { until: DateTime<Utc>, reason: String },
    Block { reason: String },
}

pub async fn preflight(
    pool: &PgPool,
    config: &PreflightConfig,
    contact: &Contact,
) -> Result<PreflightDecision, AccountError> {
    let Some(domain) = contact.company_domain.as_deref() else {
        return Ok(PreflightDecision::Proceed);
    };
    let Some(strategy) = db::strategies::by_domain(pool, domain).await? else {
        return Ok(PreflightDecision::Proceed);
    };
    if let Some(blocked) = hard_block(&strategy) {
        return Ok(blocked);
    }
    if strategy
        .coordination_flags
        .iter()
        .any(|f| f == FLAG_NEGATIVE_EVENT)
    {
        return Ok(PreflightDecision::Delay {
            until: Utc::now() + Duration::days(config.negative_event_delay_days),
            reason: "negative company event; outreach paused".into(),
        });
    }
    if strategy.coordination_flags.iter().any(|f| f == FLAG_CARPET_BOMB) {
        let touched = db::engagements::recent_outbound_contacts_for_domain(
            pool,
            domain,
            config.carpet_bomb_window_days,
        )
        .await?;
        if touched >= config.max_contacts_per_window {
            return Ok(PreflightDecision::Delay {
                until: Utc::now() + Duration::days(i64::from(config.carpet_bomb_window_days)),
                reason: format!("{touched} contacts already emailed at {domain} this window"),
            });
        }
    }
    if strategy
        .coordination_flags
        .iter()
        .any(|f| f == FLAG_NEW_CONTACT_ADVANCED)
        && strategy.stage.is_advanced()
        && is_untouched(contact)
    {
        return Ok(PreflightDecision::Modify {
            cadence: "warm-intro".into(),
            max_emails: 2,
        });
    }
    Ok(PreflightDecision::Proceed)
}

fn hard_block(strategy: &AccountStrategy) -> Option<PreflightDecision> {
    if strategy.stage == AccountStage::Customer
        || strategy.coordination_flags.iter().any(|f| f == FLAG_CONVERTED)
    {
        return Some(PreflightDecision::Block {
            reason: "account converted to customer".into(),
        });
    }
    if strategy.health == AccountHealth::Blocked {
        let mut reason = strategy.summary.clone();
        if reason.len() > 200 {
            let mut cut = 200;
            while !reason.is_char_boundary(cut) {
                cut -= 1;
            }
            reason.truncate(cut);
        }
        return Some(PreflightDecision::Block {
            reason: format!("account blocked: {reason}"),
        });
    }
    None
}

fn is_untouched(contact: &Contact) -> bool {
    matches!(
        contact.status,
        ContactStatus::New | ContactStatus::Enriched | ContactStatus::Scored
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ContactSource;

    fn strategy(stage: AccountStage, health: AccountHealth, flags: &[&str]) -> AccountStrategy {
        AccountStrategy {
            domain: "acme.io".into(),
            stage,
            health,
            coordination_flags: flags.iter().map(|f| (*f).to_string()).collect(),
            summary: "s".into(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn customer_accounts_hard_block() {
        let s = strategy(AccountStage::Customer, AccountHealth::Healthy, &[]);
        assert!(matches!(hard_block(&s), Some(PreflightDecision::Block { .. })));
        let flagged = strategy(AccountStage::Engaged, AccountHealth::Healthy, &[FLAG_CONVERTED]);
        assert!(matches!(
            hard_block(&flagged),
            Some(PreflightDecision::Block { .. })
        ));
    }

    #[test]
    fn blocked_health_blocks_with_truncated_reason() {
        let mut s = strategy(AccountStage::Engaged, AccountHealth::Blocked, &[]);
        s.summary = "é".repeat(500);
        let Some(PreflightDecision::Block { reason }) = hard_block(&s) else {
            panic!("expected block");
        };
        assert!(reason.len() < 250);
    }

    #[test]
    fn healthy_prospecting_accounts_pass_hard_blocks() {
        let s = strategy(AccountStage::Prospecting, AccountHealth::Healthy, &[]);
        assert!(hard_block(&s).is_none());
    }

    #[test]
    fn untouched_means_pre_sequence_statuses() {
        let mut contact = Contact::new(ContactSource::Csv);
        contact.status = ContactStatus::Enriched;
        assert!(is_untouched(&contact));
        contact.status = ContactStatus::InSequence;
        assert!(!is_untouched(&contact));
    }
}
