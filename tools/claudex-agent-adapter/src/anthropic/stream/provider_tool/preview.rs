use serde_json::Value;

/// Failures keep a brief hint; successes never attach tool bodies.
pub(super) const FAILED_STATUS_PREVIEW_CHAR_LIMIT: usize = 80;
const TITLE_CHAR_LIMIT: usize = 48;
const ARG_PREVIEW_CHAR_LIMIT: usize = 48;

pub(super) fn validated_provider_web_evidence(evidence: Option<&Value>) -> bool {
    let Some(evidence) = evidence else {
        return false;
    };
    let source_urls = evidence.get("source_urls").and_then(Value::as_array);
    evidence.get("provider").and_then(Value::as_str) == Some("acp")
        && evidence.get("provenance").and_then(Value::as_str)
            == Some("provider-native-tool-completion")
        && matches!(
            (
                evidence.get("kind").and_then(Value::as_str),
                evidence.get("evidence_class").and_then(Value::as_str),
            ),
            (Some("web_search"), Some("search_result_only"))
                | (Some("web_fetch"), Some("fetch_verified"))
        )
        && evidence.get("status").and_then(Value::as_str) == Some("completed")
        && evidence.get("verified").and_then(Value::as_bool) == Some(true)
        && evidence
            .get("result_summary")
            .and_then(Value::as_str)
            .is_some_and(|summary| !summary.trim().is_empty())
        && source_urls
            .map(Vec::as_slice)
            .is_some_and(valid_source_urls)
}

pub(super) fn valid_source_urls(urls: &[Value]) -> bool {
    !urls.is_empty() && urls.iter().all(valid_source_url)
}

pub(super) fn valid_source_url(url: &Value) -> bool {
    url.as_str()
        .is_some_and(|url| url.starts_with("https://") || url.starts_with("http://"))
}

pub(super) fn progress_start_line(title: &str, arguments: Option<&Value>) -> String {
    let short_title = compact_title(title);
    match argument_preview(arguments) {
        // Prefer a one-line command/path snippet; never dump full argument JSON.
        Some(detail) if !short_title.contains(detail.as_str()) => {
            format!("\n▶ {short_title}: {detail}\n")
        }
        _ => format!("\n▶ {short_title}\n"),
    }
}

pub(super) fn compact_title(title: &str) -> String {
    let trimmed = title.trim();
    // Provider titles sometimes embed the whole command; keep the tool name only.
    let head = trimmed
        .split_once(':')
        .map(|(name, _)| name.trim())
        .filter(|name| !name.is_empty() && name.len() <= TITLE_CHAR_LIMIT)
        .unwrap_or(trimmed);
    truncate_for_status(head, TITLE_CHAR_LIMIT)
}

pub(super) fn argument_preview(arguments: Option<&Value>) -> Option<String> {
    let Value::Object(map) = arguments? else {
        return None;
    };
    for key in [
        "command",
        "cmd",
        "path",
        "file_path",
        "target_file",
        "query",
        "pattern",
        "url",
        "description",
    ] {
        if let Some(value) = map.get(key).and_then(scalar_preview) {
            return Some(truncate_for_status(
                &first_line(&value),
                ARG_PREVIEW_CHAR_LIMIT,
            ));
        }
    }
    None
}

pub(super) fn scalar_preview(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

pub(super) fn failure_preview(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => first_line(text),
        Some(Value::Object(map)) => object_failure_preview(map),
        Some(_) | None => "failed".to_owned(),
    }
}

pub(super) fn object_failure_preview(map: &serde_json::Map<String, Value>) -> String {
    for key in ["error", "message", "stderr", "stdout", "reason"] {
        let Some(text) = map.get(key).and_then(Value::as_str) else {
            continue;
        };
        let line = first_line(text);
        if !line.is_empty() {
            return line;
        }
    }
    "failed".to_owned()
}

pub(super) fn terminal_already_emitted(
    ids: &mut std::collections::HashSet<String>,
    call_id: Option<&str>,
) -> bool {
    call_id.is_some_and(|call_id| !ids.insert(call_id.to_owned()))
}

pub(super) fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
}

pub(super) fn truncate_for_status(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
