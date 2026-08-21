use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

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

pub fn build_mime(params: &MimeParams<'_>) -> String {
    let mut headers = vec![
        format!("From: {} <{}>", params.from_name, params.from),
        format!("To: {}", params.to),
        format!("Subject: {}", params.subject),
        "MIME-Version: 1.0".to_string(),
    ];
    if let Some(reply_to) = params.in_reply_to {
        headers.push(format!("In-Reply-To: {reply_to}"));
    }
    if !params.references.is_empty() {
        headers.push(format!("References: {}", params.references.join(" ")));
    }
    let Some(html) = params.body_html else {
        headers.push("Content-Type: text/plain; charset=\"UTF-8\"".to_string());
        headers.push("Content-Transfer-Encoding: 8bit".to_string());
        return format!("{}\r\n\r\n{}", headers.join("\r\n"), params.body_text);
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
    lines.join("\r\n")
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

    fn params<'a>(html: Option<&'a str>) -> MimeParams<'a> {
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
        let mime = build_mime(&params(Some("<p>html body</p>")));
        assert!(mime.contains("From: Sales <sales@ours.io>"));
        assert!(mime.contains("multipart/alternative; boundary=\"b123\""));
        assert!(mime.contains("plain body"));
        assert!(mime.contains("<p>html body</p>"));
        assert!(mime.ends_with("--b123--"));
    }

    #[test]
    fn text_only_message_skips_multipart() {
        let mime = build_mime(&params(None));
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
        let mime = build_mime(&p);
        assert!(mime.contains("In-Reply-To: <m2@mail>"));
        assert!(mime.contains("References: <m1@mail> <m2@mail>"));
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
