use chrono::Utc;
use sqlx::PgPool;

use crate::db;
use crate::domain::{AccountHealth, AccountStage, AccountStrategy, Contact, Signal};
use crate::llm::{AccountAssessment, ChatMessage, LlmClient, Policy, inject};
use crate::memory::{EntityRef, MemoryClient, digest};
use crate::workflows::accounts::AccountError;

const MEMORY_DIGEST_TOKENS: usize = 300;

pub async fn evaluate_account_strategy(
    pool: &PgPool,
    memory: &MemoryClient,
    llm: &LlmClient,
    policies: &[Policy],
    domain: &str,
) -> Result<AccountStrategy, AccountError> {
    let domain = crate::domain::normalize_domain(domain);
    let Some(company) = db::companies::by_domain(pool, &domain).await? else {
        return Err(AccountError::UnknownCompany(domain));
    };
    let contacts = db::contacts::list_by_company_domain(pool, &domain).await?;
    let signals = db::signals::for_domain(pool, &domain, 10).await?;
    let entity = EntityRef::company(&domain);
    let memories = memory.recall("account history", Some(&entity), 20).await?;
    let memory_digest = digest(&memories, MEMORY_DIGEST_TOKENS);
    let prompt = assessment_prompt(
        &domain,
        company.summary.as_deref(),
        &contacts,
        &signals,
        &memory_digest,
    );
    let messages = vec![
        ChatMessage::system(inject(
            policies,
            "account strategy coordination outreach playbook",
        )),
        ChatMessage::user(prompt),
    ];
    let assessment: AccountAssessment = llm.chat_structured(messages).await?;
    let strategy = AccountStrategy {
        domain: domain.clone(),
        stage: parse_stage(&assessment.stage),
        health: parse_health(&assessment.health),
        coordination_flags: assessment.coordination_flags,
        summary: assessment.summary,
        updated_at: Utc::now(),
    };
    db::strategies::upsert(pool, &strategy).await?;
    let line = format!(
        "[STRATEGY] stage {:?}, health {:?}: {}",
        strategy.stage, strategy.health, strategy.summary
    );
    if let Err(e) = memory.memorize(&entity, &line, false).await {
        tracing::warn!(domain, error = %e, "strategy memorize failed");
    }
    Ok(strategy)
}

fn assessment_prompt(
    domain: &str,
    company_summary: Option<&str>,
    contacts: &[Contact],
    signals: &[Signal],
    memory_digest: &str,
) -> String {
    let mut sections = vec![format!(
        "Assess the outreach strategy for the account {domain}. \
         Allowed stages: prospecting, engaged, opportunity, multi_threaded, customer, dormant. \
         Allowed health: healthy, watch, blocked. \
         Allowed coordination_flags: negative_company_event, carpet_bomb_risk, \
         new_contact_at_advanced_account, account_converted. \
         Base the assessment only on the facts below."
    )];
    if let Some(summary) = company_summary {
        sections.push(format!("Company summary: {summary}"));
    }
    if contacts.is_empty() {
        sections.push("No known contacts at this account.".to_string());
    } else {
        let rollup: Vec<String> = contacts
            .iter()
            .map(|c| {
                format!(
                    "{} — {} (status {:?}, score {:?})",
                    c.email.as_deref().unwrap_or("no email"),
                    c.title.as_deref().unwrap_or("unknown title"),
                    c.status,
                    c.score,
                )
            })
            .collect();
        sections.push(format!("Contacts:\n{}", rollup.join("\n")));
    }
    if !signals.is_empty() {
        let lines: Vec<String> = signals
            .iter()
            .map(|s| format!("{:?}/{:?}: {}", s.kind, s.strength, s.summary))
            .collect();
        sections.push(format!("Signals:\n{}", lines.join("\n")));
    }
    if !memory_digest.is_empty() {
        sections.push(format!("Account memory:\n{memory_digest}"));
    }
    sections.join("\n\n")
}

fn parse_stage(raw: &str) -> AccountStage {
    match raw.to_lowercase().replace([' ', '-'], "_").as_str() {
        "engaged" => AccountStage::Engaged,
        "opportunity" => AccountStage::Opportunity,
        "multi_threaded" => AccountStage::MultiThreaded,
        "customer" => AccountStage::Customer,
        "dormant" => AccountStage::Dormant,
        _ => AccountStage::Prospecting,
    }
}

fn parse_health(raw: &str) -> AccountHealth {
    match raw.to_lowercase().as_str() {
        "blocked" => AccountHealth::Blocked,
        "watch" => AccountHealth::Watch,
        _ => AccountHealth::Healthy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_and_health_parsing_default_conservatively() {
        assert_eq!(parse_stage("Multi Threaded"), AccountStage::MultiThreaded);
        assert_eq!(parse_stage("weird"), AccountStage::Prospecting);
        assert_eq!(parse_health("BLOCKED"), AccountHealth::Blocked);
        assert_eq!(parse_health("odd"), AccountHealth::Healthy);
    }

    #[test]
    fn prompt_lists_contacts_and_signals() {
        let mut contact = Contact::new(crate::domain::ContactSource::Csv);
        contact.email = Some("jane@acme.io".into());
        contact.title = Some("VP Sales".into());
        let signal = Signal::new(
            "acme.io",
            crate::domain::SignalKind::Funding,
            crate::domain::SignalStrength::Strong,
            "raised B",
        );
        let prompt = assessment_prompt(
            "acme.io",
            Some("Makes tools"),
            &[contact],
            &[signal],
            "- old note",
        );
        assert!(prompt.contains("jane@acme.io"));
        assert!(prompt.contains("raised B"));
        assert!(prompt.contains("Account memory"));
        assert!(prompt.contains("Company summary: Makes tools"));
    }
}
