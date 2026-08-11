use serde_json::Value;

pub(super) enum NativeWebEvent<'a> {
    Started { query: &'a str },
    Completed { call_id: &'a str },
}

pub(super) fn native_web_event(event: &Value) -> Option<NativeWebEvent<'_>> {
    let item = event.pointer("/params/item")?;
    (item.get("type").and_then(Value::as_str) == Some("webSearch")).then_some(())?;
    match event.get("method").and_then(Value::as_str) {
        Some("item/started") => Some(NativeWebEvent::Started {
            query: item
                .get("query")
                .and_then(Value::as_str)
                .filter(|query| !query.trim().is_empty())
                .unwrap_or("search"),
        }),
        Some("item/completed") if native_web_succeeded(item) => item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(|call_id| NativeWebEvent::Completed { call_id }),
        _ => None,
    }
}

fn native_web_succeeded(item: &Value) -> bool {
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    !matches!(status, "failed" | "error" | "cancelled")
        && !item.get("error").is_some_and(|error| !error.is_null())
        && has_structured_retrieval_evidence(item)
}

fn has_structured_retrieval_evidence(item: &Value) -> bool {
    ["results", "sources", "evidence"]
        .into_iter()
        .filter_map(|field| item.get(field))
        .any(|value| match value {
            Value::Array(entries) => !entries.is_empty(),
            Value::Object(entries) => !entries.is_empty(),
            _ => false,
        })
}

pub(super) fn requires_verified_web_evidence(messages: &[Value], system: &Value) -> bool {
    is_dedicated_live_web_worker(system)
        || messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .filter_map(|message| message.get("content"))
            .filter_map(content_text)
            .any(|text| explicitly_requests_live_web(text.as_str()))
}

pub(super) fn is_dedicated_live_web_worker(system: &Value) -> bool {
    content_text(system).is_some_and(|text| {
        text.contains("claudex-haiku-search")
            || text.contains("Dedicated live-web retrieval worker")
            || text.contains("tools: WebSearch,WebFetch")
    })
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => Some(
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

pub(super) fn explicitly_requests_live_web(text: &str) -> bool {
    let normalized = text.to_lowercase();
    [
        "websearch",
        "webfetch",
        "web search",
        "web fetch",
        "search the web",
        "search online",
        "live web",
        "live-web",
        "web検索",
        "ウェブ検索",
        "ウェブで検索",
        "webで検索",
        "webで調査",
        "ウェブで調査",
        "インターネットで調査",
    ]
    .into_iter()
    .any(|marker| normalized.contains(marker))
}
