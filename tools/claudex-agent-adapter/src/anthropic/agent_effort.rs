use std::{collections::VecDeque, sync::Mutex, time::Instant};

use serde_json::Value;

use super::{MessagesRequest, subscription::valid_effort};

const INTENT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const MAX_PENDING_INTENTS: usize = 1_024;
const CORRELATION_TAG: &str = "claudex-agent-id";
const ADAPTER_EFFORT: &str = "claudex_effort";

struct AgentEffortIntent {
    client_user_id: Option<String>,
    prompt: String,
    effort: Option<String>,
    tool_use_id: String,
    created_at: Instant,
}

#[derive(Default)]
pub(super) struct AgentEffortIntents {
    pending: Mutex<VecDeque<AgentEffortIntent>>,
}

pub(super) enum AgentEffort {
    Unmatched,
    ConfiguredDefault,
    Explicit(String),
}

impl AgentEffortIntents {
    pub(super) fn record(
        &self,
        client_user_id: Option<&str>,
        tool_name: &str,
        tool_use_id: String,
        arguments: &Value,
    ) {
        let Some(prompt) = agent_prompt(tool_name, arguments) else {
            return;
        };
        let effort = arguments
            .get(ADAPTER_EFFORT)
            .or_else(|| arguments.get("effort"))
            .and_then(Value::as_str)
            .and_then(normalized_effort)
            .map(str::to_owned);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        if pending.len() == MAX_PENDING_INTENTS {
            pending.pop_front();
        }
        pending.push_back(AgentEffortIntent {
            client_user_id: client_user_id.map(str::to_owned),
            prompt: prompt.to_owned(),
            effort,
            tool_use_id,
            created_at: Instant::now(),
        });
    }

    pub(super) fn take(&self, request: &MessagesRequest) -> AgentEffort {
        if !is_subagent_request(&request.system) {
            return AgentEffort::Unmatched;
        }
        let client_user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        let Some(index) = pending.iter().position(|intent| {
            request_contains_prompt(&request.messages, &intent.prompt)
                && (has_correlation_marker(&intent.prompt)
                    || intent.client_user_id.as_deref() == client_user_id)
        }) else {
            return AgentEffort::Unmatched;
        };
        match pending
            .remove(index)
            .expect("matched agent effort intent")
            .effort
        {
            Some(effort) => AgentEffort::Explicit(effort),
            None => AgentEffort::ConfiguredDefault,
        }
    }

    pub(super) fn remove_tool_results<'a>(&self, tool_use_ids: impl Iterator<Item = &'a str>) {
        let ids = tool_use_ids.collect::<Vec<_>>();
        self.pending
            .lock()
            .expect("agent effort intents poisoned")
            .retain(|intent| !ids.contains(&intent.tool_use_id.as_str()));
    }
}

fn remove_expired(pending: &mut VecDeque<AgentEffortIntent>) {
    pending.retain(|intent| intent.created_at.elapsed() < INTENT_TTL);
}

fn agent_prompt<'a>(tool_name: &str, arguments: &'a Value) -> Option<&'a str> {
    (tool_name == "Agent")
        .then(|| arguments.get("prompt").and_then(Value::as_str))
        .flatten()
}

pub(super) fn prepare_arguments(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
) -> (Option<Value>, Value) {
    let mut correlated = arguments.clone();
    let Some(prompt) = agent_prompt(tool_name, arguments) else {
        return (None, correlated);
    };
    correlated["prompt"] = Value::String(format!(
        "{prompt}\n\n<{CORRELATION_TAG}>{tool_use_id}</{CORRELATION_TAG}>"
    ));
    let mut claude_arguments = correlated.clone();
    claude_arguments
        .as_object_mut()
        .expect("Agent arguments must be an object")
        .remove(ADAPTER_EFFORT);
    (Some(correlated), claude_arguments)
}

pub(super) fn tool_schema(tool_name: &str, mut schema: Value) -> Value {
    if tool_name != "Agent" {
        return schema;
    }
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    let properties = object
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    let Some(properties) = properties.as_object_mut() else {
        return schema;
    };
    properties
        .entry(ADAPTER_EFFORT.to_owned())
        .or_insert_with(|| {
            serde_json::json!({
                "type":"string",
                "enum":["low", "medium", "high", "xhigh", "max"],
                "description":"Effort for this SubAgent only. Use medium when the user says mid."
            })
        });
    schema
}

fn normalized_effort(value: &str) -> Option<&str> {
    let normalized = if value == "mid" { "medium" } else { value };
    valid_effort(normalized).then_some(normalized)
}

fn is_subagent_request(system: &Value) -> bool {
    value_texts(system).any(|text| text.contains("cc_is_subagent=true"))
}

fn request_contains_prompt(messages: &[Value], prompt: &str) -> bool {
    messages.iter().any(|message| {
        message
            .get("content")
            .is_some_and(|content| value_texts(content).any(|text| text == prompt))
    })
}

fn has_correlation_marker(prompt: &str) -> bool {
    prompt.contains(&format!("<{CORRELATION_TAG}>"))
}

fn value_texts(value: &Value) -> impl Iterator<Item = &str> {
    let direct = value.as_str().into_iter();
    let blocks = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str));
    direct.chain(blocks)
}

#[cfg(test)]
include!("agent_effort_tests.rs");
