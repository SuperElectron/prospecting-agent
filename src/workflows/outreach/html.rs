pub fn render_html(body_text: &str) -> String {
    let paragraphs: Vec<String> = body_text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", escape(p).replace('\n', "<br>")))
        .collect();
    format!(
        "<!DOCTYPE html><html><body style=\"font-family: Arial, sans-serif; font-size: 14px; \
         color: #222; line-height: 1.5;\">{}</body></html>",
        paragraphs.join("")
    )
}

fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_and_line_breaks_render() {
        let html = render_html("Hi Jane,\nquick thought.\n\nWorth a chat?");
        assert!(html.contains("<p>Hi Jane,<br>quick thought.</p>"));
        assert!(html.contains("<p>Worth a chat?</p>"));
        assert!(html.starts_with("<!DOCTYPE html>"));
    }

    #[test]
    fn html_special_characters_are_escaped() {
        let html = render_html("a < b & c > \"d\"");
        assert!(html.contains("a &lt; b &amp; c &gt; &quot;d&quot;"));
        assert!(!html.contains("a < b"));
    }

    #[test]
    fn empty_body_renders_an_empty_shell() {
        let html = render_html("   ");
        assert!(html.contains("<body"));
        assert!(!html.contains("<p>"));
    }
}
