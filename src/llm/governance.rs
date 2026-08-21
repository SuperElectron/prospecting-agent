use std::fmt::Write;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub slug: String,
    pub title: String,
    pub content: String,
    pub trigger_keywords: Vec<String>,
}

impl Policy {
    pub fn matches(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        self.trigger_keywords.iter().any(|k| lower.contains(k.as_str()))
    }
}

pub fn defaults() -> Vec<Policy> {
    vec![
        Policy {
            slug: "icp".into(),
            title: "Ideal Customer Profile".into(),
            content: "Target B2B companies of 20-2000 employees with an active sales team. \
                      Contact directors and above in sales, revenue, or business development. \
                      Disqualify existing customers, government, and companies with no outbound motion. \
                      Score: firmographic fit 40%, buying signals 30%, engagement 20%, champion potential 10%."
                .into(),
            trigger_keywords: vec![
                "icp".into(),
                "qualif".into(),
                "score".into(),
                "target".into(),
                "fit".into(),
            ],
        },
        Policy {
            slug: "voice".into(),
            title: "Outbound Voice".into(),
            content: "Confident, conversational, direct. First sentence is about them, never us. \
                      Reference one specific fact from provided context; never invent facts. \
                      No corporate filler phrases. One clear call to action per message. \
                      Sign off with first name only."
                .into(),
            trigger_keywords: vec![
                "email".into(),
                "write".into(),
                "outreach".into(),
                "message".into(),
                "tone".into(),
            ],
        },
        Policy {
            slug: "playbook".into(),
            title: "Sequence Playbook".into(),
            content: "Maximum three emails per sequence. Email 1: specific observation, value, soft ask. \
                      Email 2: new angle, medium ask. Email 3: brief, binary ask. \
                      Stop immediately on any reply. Never re-enroll opted-out contacts. \
                      LinkedIn touch only after email 1."
                .into(),
            trigger_keywords: vec![
                "sequence".into(),
                "cadence".into(),
                "follow".into(),
                "step".into(),
            ],
        },
        Policy {
            slug: "signals".into(),
            title: "Signal Definitions".into(),
            content: "Strong signals: funding round in last 90 days, 3+ open sales roles, \
                      new revenue leader in last 60 days. Moderate: sales-ops job postings, \
                      20%+ headcount growth, market expansion. Weak: general content activity."
                .into(),
            trigger_keywords: vec!["signal".into(), "funding".into(), "hiring".into(), "intent".into()],
        },
    ]
}

pub fn select<'a>(policies: &'a [Policy], prompt: &str) -> Vec<&'a Policy> {
    policies.iter().filter(|p| p.matches(prompt)).collect()
}

pub fn inject(policies: &[Policy], prompt: &str) -> String {
    let matched = select(policies, prompt);
    if matched.is_empty() {
        return prompt.to_string();
    }
    let mut sections = String::from("Follow these governance rules:\n");
    for policy in matched {
        let _ = write!(sections, "\n## {}\n{}\n", policy.title, policy.content);
    }
    format!("{sections}\n---\n{prompt}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_triggers_select_relevant_policies() {
        let policies = defaults();
        let selected = select(&policies, "Write an outreach email for this contact");
        let slugs: Vec<&str> = selected.iter().map(|p| p.slug.as_str()).collect();
        assert!(slugs.contains(&"voice"));
        assert!(!slugs.contains(&"signals"));
    }

    #[test]
    fn unrelated_prompt_injects_nothing() {
        let policies = defaults();
        let prompt = "Summarize this webpage";
        assert_eq!(inject(&policies, prompt), prompt);
    }

    #[test]
    fn injection_prepends_policy_content() {
        let policies = defaults();
        let out = inject(&policies, "Score this company against our ICP");
        assert!(out.starts_with("Follow these governance rules:"));
        assert!(out.contains("Ideal Customer Profile"));
        assert!(out.ends_with("Score this company against our ICP"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let policies = defaults();
        assert!(!select(&policies, "FUNDING news detected").is_empty());
    }
}
