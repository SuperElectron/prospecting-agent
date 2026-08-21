use chrono::Utc;
use sqlx::PgPool;

use crate::db;
use crate::domain::{
    AccountStrategy, Contact, FLAG_CARPET_BOMB, FLAG_CONVERTED, FLAG_NEGATIVE_EVENT,
    FLAG_NEW_CONTACT_ADVANCED, Signal,
};
use crate::llm::{AccountAssessment, ChatMessage, LlmClient, Policy, inject};
use crate::memory::{EntityRef, MemoryClient};
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
    let memory_digest = crate::workflows::util::best_effort_digest(
        memory,
        &entity,
        "account history",
        20,
        MEMORY_DIGEST_TOKENS,
    )
    .await;
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
        stage: assessment.stage,
        health: assessment.health,
        coordination_flags: known_flags(assessment.coordination_flags),
        summary: assessment.summary,
        updated_at: Utc::now(),
    };
    db::strategies::upsert(pool, &strategy).await?;
    let line = format!(
        "[STRATEGY] stage {:?}, health {:?}: {}",
        strategy.stage, strategy.health, strategy.summary
    );
    crate::workflows::util::best_effort_memorize(memory, &entity, &line).await;
    Ok(strategy)
}

fn known_flags(raw: Vec<String>) -> Vec<String> {
    let known = [
        FLAG_NEGATIVE_EVENT,
        FLAG_CARPET_BOMB,
        FLAG_NEW_CONTACT_ADVANCED,
        FLAG_CONVERTED,
    ];
    let mut flags: Vec<String> = Vec::new();
    for flag in raw {
        if !known.contains(&flag.as_str()) {
            tracing::warn!(flag, "llm produced an unknown coordination flag");
        } else if !flags.contains(&flag) {
            flags.push(flag);
        }
    }
    flags
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_flags_are_dropped_and_duplicates_collapse() {
        let flags = known_flags(vec![
            FLAG_CARPET_BOMB.into(),
            "do_not_contact".into(),
            FLAG_CARPET_BOMB.into(),
            FLAG_CONVERTED.into(),
        ]);
        assert_eq!(
            flags,
            vec![FLAG_CARPET_BOMB.to_string(), FLAG_CONVERTED.to_string()]
        );
    }

    #[test]
    fn assessment_schema_constrains_stage_and_health() {
        let instruction = crate::llm::schemas::schema_instruction::<AccountAssessment>();
        assert!(instruction.contains("multi_threaded"));
        assert!(instruction.contains("blocked"));
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
