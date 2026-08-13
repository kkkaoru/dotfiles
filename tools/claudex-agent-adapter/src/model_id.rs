/// Strip correlation / XML markup that models sometimes paste into `claudex_model`.
pub(crate) fn sanitize_claudex_model(model: &str) -> String {
    strip_xml_markup(&remove_correlation_tag(model))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("")
}

fn remove_correlation_tag(text: &str) -> String {
    const OPEN: &str = "<claudex-agent-id>";
    const CLOSE: &str = "</claudex-agent-id>";
    let mut remaining = text;
    let mut cleaned = String::with_capacity(text.len());
    while let Some(start) = remaining.find(OPEN) {
        cleaned.push_str(&remaining[..start]);
        let after_open = &remaining[start + OPEN.len()..];
        let Some(end) = after_open.find(CLOSE) else {
            return cleaned;
        };
        remaining = &after_open[end + CLOSE.len()..];
    }
    cleaned.push_str(remaining);
    cleaned
}

fn strip_xml_markup(text: &str) -> String {
    let mut remaining = text;
    let mut cleaned = String::with_capacity(text.len());
    while let Some(start) = remaining.find('<') {
        cleaned.push_str(&remaining[..start]);
        let after = &remaining[start + 1..];
        let Some(end) = after.find('>') else {
            return cleaned;
        };
        let tag = after[..end].trim();
        remaining = &after[end + 1..];
        if let Some(name) = opening_tag_name(tag) {
            let close = format!("</{name}>");
            if let Some(close_at) = remaining.find(&close) {
                remaining = &remaining[close_at + close.len()..];
            }
        }
    }
    cleaned.push_str(remaining);
    cleaned
}

fn opening_tag_name(tag: &str) -> Option<&str> {
    if tag.starts_with('/') || tag.starts_with('!') || tag.starts_with('?') {
        return None;
    }
    let name = tag
        .split_whitespace()
        .next()
        .unwrap_or(tag)
        .trim_end_matches('/');
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::sanitize_claudex_model;

    #[test]
    fn strips_correlation_and_other_xml_from_claudex_model() {
        assert_eq!(
            sanitize_claudex_model("grok-4.5 <claudex-agent-id>toolu_x</claudex-agent-id>"),
            "grok-4.5"
        );
        assert_eq!(
            sanitize_claudex_model("gpt-5.6-luna<status>leak</status>"),
            "gpt-5.6-luna"
        );
        assert_eq!(sanitize_claudex_model("worker-a <unclosed"), "worker-a");
        assert_eq!(sanitize_claudex_model("plain-model"), "plain-model");
    }

    #[test]
    fn sanitizes_unclosed_correlation_and_non_opening_markup() {
        assert_eq!(
            sanitize_claudex_model("grok-4.5 <claudex-agent-id>orphan"),
            "grok-4.5"
        );
        assert_eq!(
            sanitize_claudex_model("worker-a </status>keep"),
            "worker-akeep"
        );
        assert_eq!(
            sanitize_claudex_model("worker-a <!-- note -->keep"),
            "worker-akeep"
        );
        assert_eq!(
            sanitize_claudex_model("worker-a <?pi value?>keep"),
            "worker-akeep"
        );
        assert_eq!(
            sanitize_claudex_model("worker-a <status>leak"),
            "worker-aleak"
        );
    }
}
