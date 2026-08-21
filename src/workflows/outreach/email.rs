use serde::Serialize;
use sqlx::PgPool;

use crate::config::MessagingRules;
use crate::db;
use crate::domain::{Contact, Direction};
use crate::llm::{ChatMessage, LlmClient, OutreachEmail, Policy, inject};
use crate::memory::{EntityRef, MemoryClient, digest};
use crate::workflows::outreach::{OutreachError, render_html};

const MEMORY_DIGEST_TOKENS: usize = 250;
const PRIOR_EMAILS_SHOWN: usize = 5;
const GENERATION_ATTEMPTS: u32 = 2;

#[derive(Debug, Clone, Serialize)]
pub struct GeneratedEmail {
    pub subject: String,
    pub body_text: String,
    pub body_html: String,
    pub personalization_fact: String,
    pub step: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct EmailContext {
    pub step: u8,
    pub max_steps: u8,
    pub is_first_touch: bool,
}

pub async fn generate_email(
    pool: &PgPool,
    memory: &MemoryClient,
    llm: &LlmClient,
    policies: &[Policy],
    rules: &MessagingRules,
    contact: &Contact,
    context: EmailContext,
) -> Result<GeneratedEmail, OutreachError> {
    if contact.email.is_none() {
        return Err(OutreachError::NoEmail(contact.id));
    }
    let company_summary = match contact.company_domain.as_deref() {
        Some(domain) => db::companies::by_domain(pool, domain)
            .await?
            .and_then(|c| c.summary),
        None => None,
    };
    let account_memory = match contact.company_domain.as_deref() {
        Some(domain) => {
            let entity = EntityRef::company(domain);
            let items = memory
                .recall("research angles signals", Some(&entity), 20)
                .await?;
            digest(&items, MEMORY_DIGEST_TOKENS)
        }
        None => String::new(),
    };
    let contact_memory = {
        let entity = EntityRef::Contact(contact.id);
        let items = memory
            .recall("enrichment replies history", Some(&entity), 10)
            .await?;
        digest(&items, MEMORY_DIGEST_TOKENS)
    };
    let prior = db::engagements::for_contact(pool, contact.id, 20).await?;
    let prior_subjects: Vec<String> = prior
        .iter()
        .filter(|e| e.direction == Direction::Outbound)
        .filter_map(|e| e.subject.clone())
        .take(PRIOR_EMAILS_SHOWN)
        .collect();
    let base_prompt = generation_prompt(
        contact,
        context,
        company_summary.as_deref(),
        &account_memory,
        &contact_memory,
        &prior_subjects,
        rules,
    );
    let system = ChatMessage::system(inject(policies, "outreach email voice playbook messaging"));
    let mut last_violations = Vec::new();
    for attempt in 0..GENERATION_ATTEMPTS {
        let prompt = if attempt == 0 {
            base_prompt.clone()
        } else {
            format!(
                "{base_prompt}\n\nThe previous draft violated these rules: {last_violations:?}. \
                 Rewrite to satisfy every rule."
            )
        };
        let messages = vec![system.clone(), ChatMessage::user(prompt)];
        let draft: OutreachEmail = llm.chat_structured(messages).await?;
        let violations = rules.check(&draft.body, context.is_first_touch);
        if violations.is_empty() {
            return Ok(GeneratedEmail {
                body_html: render_html(&draft.body),
                subject: draft.subject,
                body_text: draft.body,
                personalization_fact: draft.personalization_fact,
                step: context.step,
            });
        }
        last_violations = violations;
    }
    Err(OutreachError::RulesViolated(last_violations))
}

fn generation_prompt(
    contact: &Contact,
    context: EmailContext,
    company_summary: Option<&str>,
    account_memory: &str,
    contact_memory: &str,
    prior_subjects: &[String],
    rules: &MessagingRules,
) -> String {
    let name = contact.first_name.as_deref().unwrap_or("there");
    let title = contact.title.as_deref().unwrap_or("unknown role");
    let company = contact.company_domain.as_deref().unwrap_or("their company");
    let max_words = if context.is_first_touch {
        rules.max_words_first_touch
    } else {
        rules.max_words_followup
    };
    let mut sections = vec![format!(
        "Write email {step} of {max} in a B2B outreach sequence to {name}, {title} at {company}. \
         Under {max_words} words. Ground every claim in the context below; reference one specific \
         fact. Plain conversational text, no placeholders, no signature block.",
        step = context.step,
        max = context.max_steps,
    )];
    if let Some(summary) = company_summary {
        sections.push(format!("Company research: {summary}"));
    }
    if !account_memory.is_empty() {
        sections.push(format!("Account memory:\n{account_memory}"));
    }
    if !contact_memory.is_empty() {
        sections.push(format!("Contact memory:\n{contact_memory}"));
    }
    if !prior_subjects.is_empty() {
        sections.push(format!(
            "Subjects already used (do not repeat these angles): {}",
            prior_subjects.join(" | ")
        ));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ContactSource;

    #[test]
    fn prompt_carries_context_and_word_budget() {
        let mut contact = Contact::new(ContactSource::Csv);
        contact.first_name = Some("Jane".into());
        contact.title = Some("VP Sales".into());
        contact.company_domain = Some("acme.io".into());
        let rules = MessagingRules::default();
        let prompt = generation_prompt(
            &contact,
            EmailContext {
                step: 2,
                max_steps: 4,
                is_first_touch: false,
            },
            Some("Devtools startup"),
            "- raised a round",
            "- opened last email",
            &["Quick question".into()],
            &rules,
        );
        assert!(prompt.contains("email 2 of 4"));
        assert!(prompt.contains("Jane, VP Sales at acme.io"));
        assert!(prompt.contains(&format!("Under {} words", rules.max_words_followup)));
        assert!(prompt.contains("Company research: Devtools startup"));
        assert!(prompt.contains("Subjects already used"));
    }

    #[test]
    fn first_touch_uses_the_larger_word_budget() {
        let contact = Contact::new(ContactSource::Csv);
        let rules = MessagingRules::default();
        let prompt = generation_prompt(
            &contact,
            EmailContext {
                step: 1,
                max_steps: 3,
                is_first_touch: true,
            },
            None,
            "",
            "",
            &[],
            &rules,
        );
        assert!(prompt.contains(&format!("Under {} words", rules.max_words_first_touch)));
        assert!(!prompt.contains("Account memory"));
    }
}
