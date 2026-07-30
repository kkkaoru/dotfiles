use serde_json::Value;

use super::super::super::{MessagesRequest, content::system_text};

pub(super) fn provider_tools(request: &MessagesRequest) -> Vec<Value> {
    if !is_live_web_retrieval_worker(request) {
        return request.tools.clone();
    }
    let filtered = request
        .tools
        .iter()
        .filter(|tool| {
            matches!(
                tool.get("name").and_then(Value::as_str),
                Some("WebSearch" | "WebFetch")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        tracing::debug!(
            "live-web retrieval worker did not receive WebSearch/WebFetch schemas; preserving the request tool set"
        );
        request.tools.clone()
    } else {
        filtered
    }
}

fn is_live_web_retrieval_worker(request: &MessagesRequest) -> bool {
    let system = system_text(&request.system);
    let messages = serde_json::to_string(&request.messages).unwrap_or_default();
    [system.as_str(), messages.as_str()]
        .into_iter()
        .any(|text| {
            text.contains("claudex-haiku-search")
                || text.contains("Dedicated live-web retrieval worker")
                || text.contains("tools: WebSearch,WebFetch")
        })
}
