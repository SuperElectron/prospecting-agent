use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OutreachEmail {
    pub subject: String,
    pub body: String,
    pub personalization_fact: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplyIntent {
    Interested,
    NotInterested,
    NotNow,
    Referral,
    OptOut,
    OutOfOffice,
    Question,
    Unclear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReplyClassification {
    pub intent: ReplyIntent,
    pub summary: String,
    pub suggested_action: String,
    pub notify_rep: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SignalAssessment {
    pub icp_fit_score: u8,
    pub signal_strength: String,
    pub recommended_action: String,
    pub reasoning: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompanyResearch {
    pub summary: String,
    pub buying_signals: Vec<String>,
    pub pain_points: Vec<String>,
    pub personalization_angles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DetectedSignal {
    pub kind: String,
    pub strength: String,
    pub summary: String,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DetectedSignals {
    pub signals: Vec<DetectedSignal>,
}

pub fn schema_instruction<T: JsonSchema>() -> String {
    let schema = schema_for!(T);
    let json = serde_json::to_string(&schema).unwrap_or_default();
    format!(
        "Respond with a single JSON object matching this JSON Schema exactly. \
         No prose before or after the JSON.\nSchema: {json}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::output::parse_structured;

    #[test]
    fn schema_instruction_names_required_fields() {
        let instruction = schema_instruction::<OutreachEmail>();
        assert!(instruction.contains("subject"));
        assert!(instruction.contains("personalization_fact"));
    }

    #[test]
    fn reply_classification_round_trips_through_parser() {
        let raw = r#"{"intent": "not_now", "summary": "busy quarter", "suggested_action": "follow up in Q4", "notify_rep": false}"#;
        let parsed: ReplyClassification = parse_structured(raw).unwrap();
        assert_eq!(parsed.intent, ReplyIntent::NotNow);
        assert!(!parsed.notify_rep);
    }

    #[test]
    fn company_research_parses_from_fenced_output() {
        let raw = "```json\n{\"summary\": \"s\", \"buying_signals\": [\"hiring\"], \"pain_points\": [], \"personalization_angles\": [\"new CRO\"]}\n```";
        let parsed: CompanyResearch = parse_structured(raw).unwrap();
        assert_eq!(parsed.buying_signals, vec!["hiring"]);
    }
}
