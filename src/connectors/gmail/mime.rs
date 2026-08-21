use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

use crate::connectors::ConnectorError;

fn header_value(name: &'static str, raw: &str) -> Result<String, ConnectorError> {
    if raw.contains('\r') || raw.contains('\n') {
        return Err(ConnectorError::HeaderInjection { header: name });
    }
    if raw.is_ascii() {
        Ok(raw.to_string())
    } else {
        Ok(format!("=?UTF-8?B?{}?=", STANDARD.encode(raw.as_bytes())))
    }
}

pub struct MimeParams<'a> {
    pub to: &'a str,
    pub from: &'a str,
    pub from_name: &'a str,
    pub subject: &'a str,
    pub body_text: &'a str,
    pub body_html: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub boundary: &'a str,
}

pub fn build_mime(params: &MimeParams<'_>) -> Result<String, ConnectorError> {
    let mut headers = vec![
        format!(
            "From: {} <{}>",
            header_value("From", params.from_name)?,
            header_value("From", params.from)?
        ),
        format!("To: {}", header_value("To", params.to)?),
        format!("Subject: {}", header_value("Subject", params.subject)?),
        "MIME-Version: 1.0".to_string(),
    ];
    if let Some(reply_to) = params.in_reply_to {
        headers.push(format!("In-Reply-To: {}", header_value("In-Reply-To", reply_to)?));
    }
    if !params.references.is_empty() {
        let joined = params.references.join(" ");
        headers.push(format!("References: {}", header_value("References", &joined)?));
    }
    let Some(html) = params.body_html else {
        headers.push("Content-Type: text/plain; charset=\"UTF-8\"".to_string());
        headers.push("Content-Transfer-Encoding: 8bit".to_string());
        return Ok(format!("{}\r\n\r\n{}", headers.join("\r\n"), params.body_text));
    };
    headers.push(format!(
        "Content-Type: multipart/alternative; boundary=\"{}\"",
        params.boundary
    ));
    let boundary = params.boundary;
    let lines = [
        headers.join("\r\n"),
        String::new(),
        format!("--{boundary}"),
        "Content-Type: text/plain; charset=\"UTF-8\"".into(),
        "Content-Transfer-Encoding: 8bit".into(),
        String::new(),
        params.body_text.into(),
        String::new(),
        format!("--{boundary}"),
        "Content-Type: text/html; charset=\"UTF-8\"".into(),
        "Content-Transfer-Encoding: 8bit".into(),
        String::new(),
        html.into(),
        String::new(),
        format!("--{boundary}--"),
    ];
    Ok(lines.join("\r\n"))
}

pub fn encode_message(mime: &str) -> String {
    URL_SAFE_NO_PAD.encode(mime.as_bytes())
}

pub fn reply_subject(original: Option<&str>, generated: &str) -> String {
    let base = original.unwrap_or(generated);
    if base.starts_with("Re: ") {
        base.to_string()
    } else {
        format!("Re: {base}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(html: Option<&str>) -> MimeParams<'_> {
        MimeParams {
            to: "jane@acme.io",
            from: "sales@ours.io",
            from_name: "Sales",
            subject: "Quick question",
            body_text: "plain body",
            body_html: html,
            in_reply_to: None,
            references: &[],
            boundary: "b123",
        }
    }

    #[test]
    fn multipart_message_carries_both_parts_and_boundary() {
        let mime = build_mime(&params(Some("<p>html body</p>"))).unwrap();
        assert!(mime.contains("From: Sales <sales@ours.io>"));
        assert!(mime.contains("multipart/alternative; boundary=\"b123\""));
        assert!(mime.contains("plain body"));
        assert!(mime.contains("<p>html body</p>"));
        assert!(mime.ends_with("--b123--"));
    }

    #[test]
    fn text_only_message_skips_multipart() {
        let mime = build_mime(&params(None)).unwrap();
        assert!(mime.contains("Content-Type: text/plain"));
        assert!(!mime.contains("multipart"));
        assert!(mime.ends_with("plain body"));
    }

    #[test]
    fn thread_headers_appear_when_present() {
        let references = vec!["<m1@mail>".to_string(), "<m2@mail>".to_string()];
        let mut p = params(None);
        p.in_reply_to = Some("<m2@mail>");
        p.references = &references;
        let mime = build_mime(&p).unwrap();
        assert!(mime.contains("In-Reply-To: <m2@mail>"));
        assert!(mime.contains("References: <m1@mail> <m2@mail>"));
    }

    #[test]
    fn crlf_in_subject_is_rejected_not_injected() {
        let mut p = params(None);
        p.subject = "hi\r\nBcc: attacker@evil.io";
        let err = build_mime(&p).unwrap_err();
        assert!(matches!(
            err,
            ConnectorError::HeaderInjection { header: "Subject" }
        ));
    }

    #[test]
    fn crlf_in_reply_headers_is_rejected() {
        let refs = vec!["<ok@mail>\r\nX-Evil: 1".to_string()];
        let mut p = params(None);
        p.references = &refs;
        assert!(build_mime(&p).is_err());
    }

    #[test]
    fn non_ascii_subject_is_rfc2047_encoded() {
        let mut p = params(None);
        p.subject = "Grüße aus Austin — schnelle Frage";
        let mime = build_mime(&p).unwrap();
        let subject_line = mime.lines().find(|l| l.starts_with("Subject:")).unwrap();
        assert!(subject_line.starts_with("Subject: =?UTF-8?B?"));
        assert!(subject_line.ends_with("?="));
        assert!(subject_line.is_ascii());
    }

    #[test]
    fn encoding_is_url_safe_base64_without_padding() {
        let encoded = encode_message("subject?>>");
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));
    }

    #[test]
    fn reply_subject_prefixes_exactly_once() {
        assert_eq!(reply_subject(Some("Hello"), "x"), "Re: Hello");
        assert_eq!(reply_subject(Some("Re: Hello"), "x"), "Re: Hello");
        assert_eq!(reply_subject(None, "Generated"), "Re: Generated");
    }
}
