use serde_json::Value;

use super::SearchResult;

// Keep these out of the parent module's LLVM mapping so coverage-branch can
// attribute the exercised paths to this file instead of treating it as missing.
#[inline(never)]
pub(super) fn append_answer_delta(event: &Value, answer: &mut String) {
    if let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) {
        answer.push_str(delta);
    }
}

#[inline(never)]
pub(super) fn is_web_search(event: &Value) -> bool {
    event.pointer("/params/item/type").and_then(Value::as_str) == Some("webSearch")
}

#[inline(never)]
pub(super) fn collect_item_results(event: &Value, output: &mut Vec<SearchResult>) {
    let Some(items) = event
        .pointer("/params/item/results")
        .and_then(Value::as_array)
    else {
        return;
    };
    output.extend(items.iter().filter_map(parse_result));
    output.sort_by(|left, right| left.url.cmp(&right.url));
    output.dedup_by(|left, right| left.url == right.url);
}

#[inline(never)]
pub(super) fn parse_result(value: &Value) -> Option<SearchResult> {
    let title = value.get("title").and_then(Value::as_str)?.trim();
    let url = value.get("url").and_then(Value::as_str)?.trim();
    if title.is_empty() || url.is_empty() {
        return None;
    }
    Some(SearchResult {
        title: title.to_owned(),
        url: url.to_owned(),
        snippet: value
            .get("snippet")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[inline(never)]
pub(super) fn extract_urls(text: &str) -> Vec<SearchResult> {
    text.split_whitespace()
        .filter_map(|token| {
            let url = token.trim_matches(|character: char| "()[]{}<>,.;\"'".contains(character));
            (url.starts_with("https://") || url.starts_with("http://")).then(|| SearchResult {
                title: url.to_owned(),
                url: url.to_owned(),
                snippet: None,
            })
        })
        .collect()
}

#[inline(never)]
pub(super) fn fallback_results(search_count: u64, answer: &str) -> Vec<SearchResult> {
    if search_count > 0 {
        extract_urls(answer)
    } else {
        Vec::new()
    }
}
