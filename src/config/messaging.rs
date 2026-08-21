use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagingRules {
    pub max_words_first_touch: usize,
    pub max_words_followup: usize,
    pub banned_phrases: Vec<String>,
    pub banned_openers: Vec<String>,
}

impl Default for MessagingRules {
    fn default() -> Self {
        Self {
            max_words_first_touch: 150,
            max_words_followup: 120,
            banned_phrases: vec![
                "synergy".into(),
                "leverage".into(),
                "touch base".into(),
                "circle back".into(),
            ],
            banned_openers: vec![
                "i hope this email finds you well".into(),
                "i'm reaching out because".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagingViolation {
    TooLong { words: usize, max: usize },
    BannedPhrase(String),
    BannedOpener(String),
}

impl MessagingRules {
    pub fn check(&self, body: &str, is_first_touch: bool) -> Vec<MessagingViolation> {
        let mut violations = Vec::new();
        let lower = body.to_lowercase();
        let words = body.split_whitespace().count();
        let max = if is_first_touch {
            self.max_words_first_touch
        } else {
            self.max_words_followup
        };
        if words > max {
            violations.push(MessagingViolation::TooLong { words, max });
        }
        for phrase in &self.banned_phrases {
            if lower.contains(phrase.as_str()) {
                violations.push(MessagingViolation::BannedPhrase(phrase.clone()));
            }
        }
        let opener: String = lower.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
        for banned in &self.banned_openers {
            if opener.starts_with(banned.as_str()) {
                violations.push(MessagingViolation::BannedOpener(banned.clone()));
            }
        }
        violations
    }

    pub fn passes(&self, body: &str, is_first_touch: bool) -> bool {
        self.check(body, is_first_touch).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_short_email_passes() {
        let rules = MessagingRules::default();
        assert!(rules.passes(
            "Saw your Series B announcement. Worth a quick look at how we help?",
            true
        ));
    }

    #[test]
    fn banned_opener_is_flagged() {
        let rules = MessagingRules::default();
        let v = rules.check("I hope this email finds you well. We sell things.", true);
        assert!(v.iter().any(|x| matches!(x, MessagingViolation::BannedOpener(_))));
    }

    #[test]
    fn banned_phrase_is_flagged_case_insensitively() {
        let rules = MessagingRules::default();
        let v = rules.check("Let's Leverage our Synergy.", true);
        assert_eq!(
            v.iter()
                .filter(|x| matches!(x, MessagingViolation::BannedPhrase(_)))
                .count(),
            2
        );
    }

    #[test]
    fn word_cap_differs_by_touch_number() {
        let rules = MessagingRules::default();
        let body = "word ".repeat(130);
        assert!(rules.passes(&body, true));
        assert!(!rules.passes(&body, false));
    }
}
