use serde::Serialize;
use sqlx::PgPool;

use crate::clients::tavily::{SearchOptions, SearchResponse, TavilyClient};
use crate::db;
use crate::llm::{ChatMessage, CompanyResearch, LlmClient, Policy, inject};
use crate::memory::{EntityRef, MemoryClient};
use crate::workflows::research::ResearchError;

const MAX_RESULT_CHARS: usize = 700;

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
    let Some(mut company) = db::companies::by_domain(pool, &domain).await? else {
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
    company.summary = Some(research.summary.clone());
    db::companies::upsert(pool, &company).await?;
    let entity = EntityRef::company(&domain);
    let line = format!("[RESEARCH] {}", research.summary);
    if let Err(e) = memory.memorize(&entity, &line, false).await {
        tracing::warn!(domain, error = %e, "research summary memorize failed");
    }
    for angle in &research.personalization_angles {
        if let Err(e) = memory.memorize(&entity, &format!("[ANGLE] {angle}"), false).await {
            tracing::warn!(domain, error = %e, "personalization angle memorize failed");
        }
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
         Use only the web results below; do not invent facts."
    )];
    if let Some(answer) = &results.answer {
        sections.push(format!("Search summary: {answer}"));
    }
    for (index, result) in results.results.iter().enumerate() {
        let mut content = result.content.clone();
        if content.len() > MAX_RESULT_CHARS {
            let mut cut = MAX_RESULT_CHARS;
            while !content.is_char_boundary(cut) {
                cut -= 1;
            }
            content.truncate(cut);
        }
        sections.push(format!(
            "Result {n} — {title} ({url}): {content}",
            n = index + 1,
            title = result.title,
            url = result.url,
        ));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::tavily::SearchResult;

    #[test]
    fn prompt_carries_sources_and_truncates_on_char_boundaries() {
        let results = SearchResponse {
            answer: Some("Acme raised money.".into()),
            results: vec![SearchResult {
                title: "Funding".into(),
                url: "https://news.example.com/a".into(),
                content: "é".repeat(MAX_RESULT_CHARS),
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
