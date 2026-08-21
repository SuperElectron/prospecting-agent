use serde::de::DeserializeOwned;

use crate::llm::client::LlmError;

pub fn extract_json(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    extract_fenced(trimmed)
        .and_then(balanced_json)
        .or_else(|| balanced_json(trimmed))
}

fn extract_fenced(raw: &str) -> Option<&str> {
    let start = raw.find("```")?;
    let after = &raw[start + 3..];
    let body_start = after.find('\n')? + 1;
    let body = &after[body_start..];
    let end = body.find("```")?;
    Some(body[..end].trim())
}

fn balanced_json(raw: &str) -> Option<&str> {
    let mut search_from = 0;
    while let Some(rel) = raw[search_from..].find(['{', '[']) {
        let open = search_from + rel;
        if let Some(found) = balanced_json_at(raw, open) {
            return Some(found);
        }
        search_from = open + 1;
    }
    None
}

fn balanced_json_at(raw: &str, open: usize) -> Option<&str> {
    let opener = raw.as_bytes()[open];
    let closer = if opener == b'{' { b'}' } else { b']' };
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, b) in raw.bytes().enumerate().skip(open) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            _ if b == opener => depth += 1,
            _ if b == closer => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[open..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn parse_structured<T: DeserializeOwned>(raw: &str) -> Result<T, LlmError> {
    let json = extract_json(raw).ok_or_else(|| LlmError::Parse("no JSON object found in output".into()))?;
    serde_json::from_str(json).map_err(|e| LlmError::Parse(format!("{e}; extracted: {json}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Sample {
        subject: String,
        score: u8,
    }

    #[test]
    fn parses_bare_json() {
        let s: Sample = parse_structured(r#"{"subject": "hi", "score": 7}"#).unwrap();
        assert_eq!(s.score, 7);
    }

    #[test]
    fn parses_fenced_json_with_preamble() {
        let raw =
            "Sure! Here is the result:\n```json\n{\"subject\": \"hi\", \"score\": 7}\n```\nHope that helps.";
        let s: Sample = parse_structured(raw).unwrap();
        assert_eq!(s.subject, "hi");
    }

    #[test]
    fn parses_json_embedded_in_chatter() {
        let raw = "The answer is {\"subject\": \"q\", \"score\": 3} as requested.";
        let s: Sample = parse_structured(raw).unwrap();
        assert_eq!(s.score, 3);
    }

    #[test]
    fn braces_inside_strings_do_not_break_balancing() {
        let raw = r#"{"subject": "curly } brace", "score": 1}"#;
        let s: Sample = parse_structured(raw).unwrap();
        assert_eq!(s.subject, "curly } brace");
    }

    #[test]
    fn arrays_extract_too() {
        let raw = "list: [1, 2, 3] done";
        let v: Vec<u8> = parse_structured(raw).unwrap();
        assert_eq!(v, vec![1, 2, 3]);
    }

    #[test]
    fn json_after_a_non_json_fence_is_still_found() {
        let raw = "```\nnot json\n```\n{\"subject\": \"x\", \"score\": 2}";
        let s: Sample = parse_structured(raw).unwrap();
        assert_eq!(s.score, 2);
    }

    #[test]
    fn unbalanced_prose_brace_before_real_json_is_skipped() {
        let raw = "the format is { key: value ... anyway: {\"subject\": \"y\", \"score\": 4}";
        let s: Sample = parse_structured(raw).unwrap();
        assert_eq!(s.score, 4);
    }

    #[test]
    fn missing_json_is_a_parse_error() {
        let r: Result<Sample, _> = parse_structured("no json here at all");
        assert!(r.is_err());
    }

    #[test]
    fn wrong_shape_reports_serde_error() {
        let r: Result<Sample, _> = parse_structured(r#"{"unexpected": true}"#);
        assert!(r.is_err());
    }
}
