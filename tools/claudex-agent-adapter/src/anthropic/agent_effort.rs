use std::{collections::VecDeque, sync::Mutex, time::Instant};

use serde_json::Value;

use super::{MessagesRequest, subscription::valid_effort};

const INTENT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const MAX_PENDING_INTENTS: usize = 1_024;
const CORRELATION_TAG: &str = "claudex-agent-id";
const ADAPTER_EFFORT: &str = "claudex_effort";
const ADAPTER_MODEL: &str = "claudex_model";

#[derive(Clone)]
struct AgentEffortIntent {
    client_user_id: Option<String>,
    prompt: String,
    effort: Option<String>,
    model_override: String,
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

pub(super) struct AgentIntent {
    pub(super) effort: AgentEffort,
    pub(super) model_override: Option<String>,
    pub(super) is_subagent: bool,
}

impl AgentIntent {
    fn unmatched(is_subagent: bool) -> Self {
        Self {
            effort: AgentEffort::Unmatched,
            model_override: None,
            is_subagent,
        }
    }
}

impl AgentEffortIntents {
    #[cfg(test)]
    pub(super) fn record(
        &self,
        client_user_id: Option<&str>,
        tool_name: &str,
        tool_use_id: String,
        parent_model: &str,
        arguments: &Value,
    ) {
        self.record_from_user_messages(
            client_user_id,
            tool_name,
            tool_use_id,
            parent_model,
            arguments,
            &[],
        );
    }

    pub(super) fn record_from_user_messages(
        &self,
        client_user_id: Option<&str>,
        tool_name: &str,
        tool_use_id: String,
        parent_model: &str,
        arguments: &Value,
        user_messages: &[Value],
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
        let requested_model = requested_model(arguments);
        let explicit_model = requested_model.filter(|model| {
            message_texts(user_messages).any(|text| contains_model_id(text, model))
        });
        if requested_model.is_some() && explicit_model.is_none() {
            tracing::debug!(
                requested_model = requested_model.unwrap_or_default(),
                %parent_model,
                "ignored SubAgent model not explicitly present in current user input"
            );
        }
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        if pending.len() == MAX_PENDING_INTENTS {
            pending.pop_front();
        }
        pending.push_back(AgentEffortIntent {
            client_user_id: client_user_id.map(str::to_owned),
            prompt: prompt.to_owned(),
            effort,
            model_override: explicit_model.unwrap_or(parent_model).to_owned(),
            tool_use_id,
            created_at: Instant::now(),
        });
    }

    pub(super) fn take(&self, request: &MessagesRequest) -> AgentIntent {
        if !is_subagent_request(request) {
            return AgentIntent::unmatched(false);
        }
        let client_user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        let Some(index) = pending.iter().position(|intent| {
            request_matches_intent(&request.messages, intent)
                && (has_correlation_marker(&intent.prompt)
                    || intent.client_user_id.as_deref() == client_user_id)
        }) else {
            return AgentIntent::unmatched(true);
        };
        let intent = if has_correlation_marker(&pending[index].prompt) {
            pending[index].clone()
        } else {
            pending.remove(index).expect("matched agent intent")
        };
        let effort = match intent.effort {
            Some(effort) => AgentEffort::Explicit(effort),
            None => AgentEffort::ConfiguredDefault,
        };
        AgentIntent {
            effort,
            model_override: Some(intent.model_override),
            is_subagent: true,
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

fn requested_model(arguments: &Value) -> Option<&str> {
    arguments
        .get(ADAPTER_MODEL)
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
}

fn contains_model_id(text: &str, model: &str) -> bool {
    text.match_indices(model).any(|(start, _)| {
        let end = start + model.len();
        let before_is_boundary = text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_model_id_character(character));
        let after_is_boundary = model_id_ends_at_boundary(&text[end..]);
        before_is_boundary && after_is_boundary
    })
}

fn model_id_ends_at_boundary(remaining: &str) -> bool {
    let mut characters = remaining.chars();
    match characters.next() {
        None => true,
        Some(character) if !is_model_id_character(character) => true,
        Some(character @ ('.' | ':')) => characters
            .next()
            .is_none_or(|next| !is_model_id_character(next) || next == character),
        Some(_) => false,
    }
}

fn is_model_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':' | '/')
}

#[cfg(test)]
pub(super) fn prepare_arguments(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
) -> (Option<Value>, Value) {
    prepare_arguments_for_user(tool_name, tool_use_id, arguments, &[])
}

pub(super) fn prepare_arguments_for_user(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
    user_messages: &[Value],
) -> (Option<Value>, Value) {
    let mut correlated = arguments.clone();
    let Some(prompt) = agent_prompt(tool_name, arguments) else {
        return (None, correlated);
    };
    correlated["prompt"] = Value::String(format!(
        "{prompt}\n\n<{CORRELATION_TAG}>{tool_use_id}</{CORRELATION_TAG}>"
    ));
    let mut claude_arguments = correlated.clone();
    let public = claude_arguments
        .as_object_mut()
        .expect("Agent arguments must be an object");
    public.remove(ADAPTER_EFFORT);
    public.remove(ADAPTER_MODEL);
    public.remove("model");
    if public
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| !active_user_supplied_name(user_messages, name))
    {
        public.remove("name");
    }
    (Some(correlated), claude_arguments)
}

fn active_user_supplied_name(messages: &[Value], name: &str) -> bool {
    messages
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
        .find(|text| {
            !text.contains("<agent-message")
                && !text.contains("<teammate-message")
                && !text.starts_with("Another Claude session sent a message")
        })
        .is_some_and(|text| explicitly_names_agent(text, name))
}

fn explicitly_names_agent(text: &str, name: &str) -> bool {
    [
        format!("`{name}`"),
        format!("\"{name}\""),
        format!("@{name}"),
        format!("name {name}"),
        format!("names {name}"),
        format!("named {name}"),
        format!("named teammate {name}"),
        format!("名前を{name}"),
        format!("{name}という名前"),
    ]
    .iter()
    .any(|pattern| text.contains(pattern))
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
    if let Some(name) = properties.get_mut("name").and_then(Value::as_object_mut) {
        name.insert(
            "description".to_owned(),
            Value::String("Persistent mailbox teammate name. Omit for ordinary SubAgents and parallel delegation. Set only to an exact teammate name explicitly supplied by the active user; never invent one.".to_owned()),
        );
    }
    properties
        .entry(ADAPTER_EFFORT.to_owned())
        .or_insert_with(|| {
            serde_json::json!({
                "type":"string",
                "enum":["low", "medium", "high", "xhigh", "max"],
                "description":"Effort for this SubAgent only. Use medium when the user says mid."
            })
        });
    properties
        .entry(ADAPTER_MODEL.to_owned())
        .or_insert_with(|| {
            serde_json::json!({
                "type":"string",
                "minLength":1,
                "description":"Exact model ID explicitly requested by the user for this SubAgent. IDs beginning with gpt or grok use the corresponding routed provider. Omit it to inherit the current session model."
            })
        });
    schema
}

fn normalized_effort(value: &str) -> Option<&str> {
    let normalized = if value == "mid" { "medium" } else { value };
    valid_effort(normalized).then_some(normalized)
}

fn is_subagent_request(request: &MessagesRequest) -> bool {
    value_texts(&request.system).any(|text| text.contains("cc_is_subagent=true"))
        || request
            .messages
            .iter()
            .filter_map(|message| message.get("content"))
            .flat_map(value_texts)
            .any(has_correlation_marker)
}

fn request_contains_prompt(messages: &[Value], prompt: &str) -> bool {
    message_texts(messages).any(|text| text == prompt)
}

fn request_matches_intent(messages: &[Value], intent: &AgentEffortIntent) -> bool {
    if has_correlation_marker(&intent.prompt) {
        let marker = format!(
            "<{CORRELATION_TAG}>{}</{CORRELATION_TAG}>",
            intent.tool_use_id
        );
        return message_texts(messages).any(|text| text.contains(&marker));
    }
    request_contains_prompt(messages, &intent.prompt)
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

fn message_texts(messages: &[Value]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
}

#[cfg(test)]
include!("agent_effort_tests.rs");
