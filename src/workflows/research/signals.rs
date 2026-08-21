use serde::Serialize;
use sqlx::PgPool;

use crate::clients::tavily::{SearchOptions, SearchResponse, TavilyClient, Topic};
use crate::domain::{Signal, SignalKind, SignalStrength};
use crate::llm::{ChatMessage, DetectedSignals, LlmClient, Policy, inject};
use crate::memory::MemoryClient;
use crate::workflows::research::{ResearchError, UNTRUSTED_NOTE, truncated};
use crate::workflows::sync::ingest_signal;

#[derive(Debug, Default, Serialize)]
pub struct SignalDetectionReport {
    pub detected: u64,
    pub ingested: u64,
    pub unknown_kind: u64,
    pub duplicate: u64,
}

pub async fn detect_signals(
    pool: &PgPool,
    memory: &MemoryClient,
    tavily: &TavilyClient,
    llm: &LlmClient,
    policies: &[Policy],
    domain: &str,
) -> Result<SignalDetectionReport, ResearchError> {
    let domain = crate::domain::normalize_domain(domain);
    if crate::db::companies::by_domain(pool, &domain).await?.is_none() {
        return Err(ResearchError::UnknownCompany(domain));
    }
    let query = format!("{domain} funding OR hiring OR expansion OR leadership announcement");
    let options = SearchOptions {
        topic: Topic::News,
        ..SearchOptions::default()
    };
    let results = tavily.search(&query, &options).await?;
    if results.results.is_empty() {
        return Ok(SignalDetectionReport::default());
    }
    let prompt = detection_prompt(&domain, &results);
    let messages = vec![
        ChatMessage::system(inject(policies, "buying signals detection scoring")),
        ChatMessage::user(prompt),
    ];
    let detected: DetectedSignals = llm.chat_structured(messages).await?;
    let known_urls: Vec<&str> = results.results.iter().map(|r| r.url.as_str()).collect();
    let existing = crate::db::signals::for_domain(pool, &domain, 50).await?;
    let mut report = SignalDetectionReport {
        detected: detected.signals.len() as u64,
        ingested: 0,
        unknown_kind: 0,
        duplicate: 0,
    };
    for raw in detected.signals {
        let Some(kind) = parse_kind(&raw.kind) else {
            tracing::debug!(kind = raw.kind, "llm produced an unmapped signal kind");
            report.unknown_kind += 1;
            continue;
        };
        let source_url = raw.source_url.filter(|url| known_urls.contains(&url.as_str()));
        if is_duplicate(&existing, kind, source_url.as_deref()) {
            report.duplicate += 1;
            continue;
        }
        let strength = parse_strength(&raw.strength);
        let mut signal = Signal::new(&domain, kind, strength, raw.summary);
        signal.source_url = source_url;
        ingest_signal(pool, memory, &signal).await?;
        report.ingested += 1;
    }
    Ok(report)
}

fn detection_prompt(domain: &str, results: &SearchResponse) -> String {
    let mut sections = vec![format!(
        "Identify buying signals for the company at {domain} from the news results below. \
         Allowed kinds: funding, hiring, leadership_change, expansion, content_mention, \
         tech_adoption, other. Allowed strengths: weak, moderate, strong. \
         Only report signals supported by the results; return an empty list otherwise. \
         {UNTRUSTED_NOTE}"
    )];
    for (index, result) in results.results.iter().enumerate() {
        sections.push(format!(
            "Result {n} — {title} ({url}) {date}:\n<web_result>\n{content}\n</web_result>",
            n = index + 1,
            title = result.title,
            url = result.url,
            date = result.published_date.as_deref().unwrap_or("undated"),
            content = truncated(&result.content),
        ));
    }
    sections.join("\n\n")
}

fn is_duplicate(existing: &[Signal], kind: SignalKind, source_url: Option<&str>) -> bool {
    existing.iter().any(|signal| {
        signal.kind == kind
            && match (source_url, signal.source_url.as_deref()) {
                (Some(new_url), Some(old_url)) => new_url == old_url,
                _ => (chrono::Utc::now() - signal.detected_at).num_days() < 7,
            }
    })
}

fn parse_kind(raw: &str) -> Option<SignalKind> {
    match raw.to_lowercase().replace([' ', '-'], "_").as_str() {
        "funding" => Some(SignalKind::Funding),
        "hiring" => Some(SignalKind::Hiring),
        "leadership_change" => Some(SignalKind::LeadershipChange),
        "expansion" => Some(SignalKind::Expansion),
        "content_mention" => Some(SignalKind::ContentMention),
        "tech_adoption" => Some(SignalKind::TechAdoption),
        "other" => Some(SignalKind::Other),
        _ => None,
    }
}

fn parse_strength(raw: &str) -> SignalStrength {
    match raw.to_lowercase().as_str() {
        "strong" => SignalStrength::Strong,
        "moderate" | "medium" => SignalStrength::Moderate,
        _ => SignalStrength::Weak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_tolerates_case_and_spacing_but_rejects_junk() {
        assert_eq!(parse_kind("Funding"), Some(SignalKind::Funding));
        assert_eq!(
            parse_kind("leadership change"),
            Some(SignalKind::LeadershipChange)
        );
        assert_eq!(parse_kind("Tech-Adoption"), Some(SignalKind::TechAdoption));
        assert_eq!(parse_kind("acquisition rumor"), None);
    }

    #[test]
    fn duplicates_match_on_source_url_or_recent_same_kind() {
        let mut with_url = Signal::new("acme.io", SignalKind::Funding, SignalStrength::Strong, "raise");
        with_url.source_url = Some("https://n.example.com/a".into());
        assert!(is_duplicate(
            std::slice::from_ref(&with_url),
            SignalKind::Funding,
            Some("https://n.example.com/a")
        ));
        assert!(is_duplicate(&[with_url.clone()], SignalKind::Funding, None));
        assert!(!is_duplicate(&[with_url], SignalKind::Hiring, None));
    }

    #[test]
    fn strength_parsing_defaults_weak() {
        assert_eq!(parse_strength("STRONG"), SignalStrength::Strong);
        assert_eq!(parse_strength("medium"), SignalStrength::Moderate);
        assert_eq!(parse_strength("meh"), SignalStrength::Weak);
    }
}
