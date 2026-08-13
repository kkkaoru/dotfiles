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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_helpers_cover_missing_and_invalid_values() {
        let mut answer = String::new();
        append_answer_delta(&serde_json::json!({}), &mut answer);
        append_answer_delta(
            &serde_json::json!({"params": {"delta": "answer"}}),
            &mut answer,
        );
        assert_eq!(answer, "answer");

        let mut results = Vec::new();
        collect_item_results(&serde_json::json!({}), &mut results);
        collect_item_results(
            &serde_json::json!({"params": {"item": {"results": [
                {"title": " ", "url": "https://empty.test"},
                {"title": "valid", "url": " "}
            ]}}}),
            &mut results,
        );
        assert!(results.is_empty());
        assert!(!is_web_search(&serde_json::json!({})));
        assert!(is_web_search(
            &serde_json::json!({"params": {"item": {"type": "webSearch"}}})
        ));
    }

    #[test]
    fn url_and_fallback_parsing_cover_both_protocols() {
        let results = extract_urls("https://one.test, (http://two.test). ordinary");
        assert_eq!(results.len(), 2);
        assert!(fallback_results(0, "https://ignored.test").is_empty());
        assert_eq!(fallback_results(1, "source https://one.test").len(), 1);
        assert!(parse_result(&serde_json::json!({"title": "x"})).is_none());
        assert_eq!(
            parse_result(&serde_json::json!({
                "title": " title ", "url": " https://source.test ", "snippet": "text"
            }))
            .expect("valid result")
            .url,
            "https://source.test"
        );
    }
}
