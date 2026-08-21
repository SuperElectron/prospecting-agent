use serde::Serialize;
use sqlx::PgPool;

use crate::clients::tavily::{SearchOptions, SearchResponse, TavilyClient};
use crate::db;
use crate::llm::{ChatMessage, CompanyResearch, LlmClient, Policy, inject};
use crate::memory::{EntityRef, MemoryClient};
use crate::workflows::research::{ResearchError, UNTRUSTED_NOTE, truncated};

#[derive(Debug, Serialize)]
pub struct ResearchOutcome {
    pub domain: String,
    pub research: CompanyResearch,
    pub sources: Vec<String>,
}

pub async fn research_company(
    pool: &PgPool,
    memory: &MemoryClient,
    tavily: &TavilyClient,
    llm: &LlmClient,
    policies: &[Policy],
    domain: &str,
) -> Result<ResearchOutcome, ResearchError> {
    let domain = crate::domain::normalize_domain(domain);
    let Some(company) = db::companies::by_domain(pool, &domain).await? else {
        return Err(ResearchError::UnknownCompany(domain));
    };
    let display_name = company.name.clone().unwrap_or_else(|| domain.clone());
    let query = format!("{display_name} {domain} company product funding hiring news");
    let results = tavily.search(&query, &SearchOptions::default()).await?;
    let prompt = research_prompt(&display_name, &domain, &results);
    let messages = vec![
        ChatMessage::system(inject(
            policies,
            "company research for outreach targeting icp scoring",
        )),
        ChatMessage::user(prompt),
    ];
    let research: CompanyResearch = llm.chat_structured(messages).await?;
    let updated = crate::domain::Company {
        summary: Some(research.summary.clone()),
        updated_at: chrono::Utc::now(),
        ..company
    };
    db::companies::upsert_enrichment(pool, &updated).await?;
    let entity = EntityRef::company(&domain);
    let line = format!("[RESEARCH] {}", research.summary);
    crate::workflows::util::best_effort_memorize(memory, &entity, &line).await;
    for angle in &research.personalization_angles {
        crate::workflows::util::best_effort_memorize(memory, &entity, &format!("[ANGLE] {angle}")).await;
    }
    let sources = results.results.iter().map(|r| r.url.clone()).collect();
    Ok(ResearchOutcome {
        domain,
        research,
        sources,
    })
}

fn research_prompt(name: &str, domain: &str, results: &SearchResponse) -> String {
    let mut sections = vec![format!(
        "Research the company {name} ({domain}) for B2B sales outreach. \
         Use only the web results below; do not invent facts. {UNTRUSTED_NOTE}"
    )];
    if let Some(answer) = &results.answer {
        sections.push(format!("Search summary: {answer}"));
    }
    for (index, result) in results.results.iter().enumerate() {
        sections.push(format!(
            "Result {n} — {title} ({url}):\n<web_result>\n{content}\n</web_result>",
            n = index + 1,
            title = result.title,
            url = result.url,
            content = truncated(&result.content),
        ));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::tavily::SearchResult;
    use crate::workflows::research::MAX_RESULT_BYTES;

    #[test]
    fn prompt_carries_sources_and_truncates_on_char_boundaries() {
        let results = SearchResponse {
            answer: Some("Acme raised money.".into()),
            results: vec![SearchResult {
                title: "Funding".into(),
                url: "https://news.example.com/a".into(),
                content: "é".repeat(MAX_RESULT_BYTES),
                score: 0.9,
                published_date: None,
            }],
        };
        let prompt = research_prompt("Acme", "acme.io", &results);
        assert!(prompt.contains("Acme (acme.io)"));
        assert!(prompt.contains("Search summary: Acme raised money."));
        assert!(prompt.contains("https://news.example.com/a"));
    }

    #[test]
    fn prompt_without_results_still_instructs_grounding() {
        let results = SearchResponse {
            answer: None,
            results: vec![],
        };
        let prompt = research_prompt("Acme", "acme.io", &results);
        assert!(prompt.contains("do not invent facts"));
    }
}
