use std::{collections::VecDeque, sync::Mutex, time::Instant};

use serde_json::Value;

mod background_launch;
mod model;
mod terminal;
#[cfg(test)]
pub(super) use super::agent_route_validation::validate_routed_agent_arguments;
pub(super) use super::agent_route_validation::validate_routed_agent_arguments_with_catalog;
pub(super) use model::{disabled_subagent_model, is_agent_tool, requested_model};
use terminal::terminal_task_notification_ids;

pub(super) use super::AgentEffortRecord;
pub(super) use super::agent_effort_matching::is_subagent_request;
use super::{
    MessagesRequest,
    agent_effort_matching::{
        has_correlation_marker, request_matches_intent_with_system, value_texts,
    },
    agent_intent_store::{persistence_snapshot, remove_expired, unix_seconds},
    subscription::valid_effort,
};

pub(super) const INTENT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);
pub(super) const MAX_PENDING_INTENTS: usize = 1_024;
const ADAPTER_EFFORT: &str = "claudex_effort";
const ADAPTER_MODEL: &str = "claudex_model";
const IMPLICIT_MODEL: &str = "claudex_implicit_model";
#[derive(Clone)]
pub(super) struct AgentEffortIntent {
    pub(super) client_user_id: Option<String>,
    pub(super) prompt: String,
    pub(super) correlated: bool,
    pub(super) effort: Option<String>,
    pub(super) model_override: Option<String>,
    pub(super) model_is_inherited: bool,
    pub(super) run_in_background: bool,
    pub(super) tool_use_id: String,
    pub(super) created_at: Instant,
    pub(super) created_unix_seconds: u64,
}
pub(super) struct AgentEffortIntents {
    pub(super) pending: Mutex<VecDeque<AgentEffortIntent>>,
    pub(super) store: Option<super::agent_intent_store::AgentIntentStore>,
}
pub(super) enum AgentEffort {
    Unmatched,
    ConfiguredDefault,
    Explicit(String),
}
pub(super) struct AgentIntent {
    pub(super) effort: AgentEffort,
    pub(super) model_override: Option<String>,
    pub(super) model_is_inherited: bool,
    pub(super) run_in_background: bool,
    pub(super) is_subagent: bool,
    pub(super) matched: bool,
}
impl AgentIntent {
    fn unmatched(is_subagent: bool) -> Self {
        Self {
            effort: AgentEffort::Unmatched,
            model_override: None,
            model_is_inherited: false,
            run_in_background: false,
            is_subagent,
            matched: false,
        }
    }
}
impl AgentEffortIntents {
    pub(super) fn record_from_user_messages(
        &self,
        input: AgentEffortRecord<'_>,
        model_catalog: Option<&crate::provider_config::ModelCatalog>,
    ) {
        let AgentEffortRecord {
            client_user_id,
            tool_name,
            tool_use_id,
            parent_model,
            arguments,
            user_messages,
            system,
        } = input;
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
            authorized_model(arguments, user_messages, system, model_catalog, model)
        });
        if requested_model.is_some() && explicit_model.is_none() {
            tracing::debug!(
                requested_model = requested_model.unwrap_or_default(),
                %parent_model,
                "ignored unrouted SubAgent model not explicitly present in current user input"
            );
        }
        let model_is_inherited = explicit_model.is_some_and(|model| {
            arguments.get(IMPLICIT_MODEL).and_then(Value::as_str) == Some(model)
        });
        let run_in_background =
            arguments.get("run_in_background").and_then(Value::as_bool) == Some(true);
        let correlated = has_correlation_marker(prompt);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        if pending.len() == MAX_PENDING_INTENTS {
            pending.pop_front();
        }
        pending.push_back(AgentEffortIntent {
            client_user_id: client_user_id.map(str::to_owned),
            prompt: if correlated {
                String::new()
            } else {
                prompt.to_owned()
            },
            correlated,
            effort,
            model_override: explicit_model.map(str::to_owned),
            model_is_inherited,
            run_in_background,
            tool_use_id,
            created_at: Instant::now(),
            created_unix_seconds: unix_seconds(),
        });
        let snapshot = persistence_snapshot(&pending);
        drop(pending);
        self.persist(snapshot);
    }
    pub(super) fn take(&self, request: &MessagesRequest) -> AgentIntent {
        if !is_subagent_request(request) {
            return AgentIntent::unmatched(false);
        }
        let client_user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        let index = pending
            .iter()
            .position(|intent| {
                request_matches_intent_with_system(&request.system, &request.messages, intent)
                    && (intent.correlated || intent.client_user_id.as_deref() == client_user_id)
            })
            .or_else(|| unique_correlated_candidate(&pending, client_user_id));
        let Some(index) = index else {
            return AgentIntent::unmatched(true);
        };
        let intent = if pending[index].correlated {
            let intent = pending
                .remove(index)
                .expect("matched correlated agent intent");
            pending.push_back(intent.clone());
            intent
        } else {
            pending.remove(index).expect("matched agent intent")
        };
        let effort = match intent.effort {
            Some(effort) => AgentEffort::Explicit(effort),
            None => AgentEffort::ConfiguredDefault,
        };
        let result = AgentIntent {
            effort,
            model_override: intent.model_override,
            model_is_inherited: intent.model_is_inherited,
            run_in_background: intent.run_in_background,
            is_subagent: true,
            matched: true,
        };
        let snapshot = persistence_snapshot(&pending);
        drop(pending);
        self.persist(snapshot);
        result
    }

    pub(super) fn remove_tool_results<'a>(&self, tool_use_ids: impl Iterator<Item = &'a str>) {
        let ids = tool_use_ids.collect::<Vec<_>>();
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        pending.retain(|intent| intent.correlated || !ids.contains(&intent.tool_use_id.as_str()));
        let snapshot = persistence_snapshot(&pending);
        drop(pending);
        self.persist(snapshot);
    }

    pub(super) fn retire_terminal_task_notifications(&self, request: &MessagesRequest) {
        let ids = terminal_task_notification_ids(&request.messages);
        if ids.is_empty() {
            return;
        }
        let client_user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        pending.retain(|intent| retain_terminal_intent(intent, &ids, client_user_id));
        let snapshot = persistence_snapshot(&pending);
        drop(pending);
        self.persist(snapshot);
    }
}

fn authorized_model(
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
    model_catalog: Option<&crate::provider_config::ModelCatalog>,
    model: &str,
) -> bool {
    match model_catalog {
        Some(catalog) => super::agent_routing::model_is_authorized_with_catalog(
            arguments,
            user_messages,
            system,
            catalog,
            model,
        ),
        None => super::agent_routing::model_is_authorized(arguments, user_messages, system, model),
    }
}

fn unique_correlated_candidate(
    pending: &VecDeque<AgentEffortIntent>,
    client_user_id: Option<&str>,
) -> Option<usize> {
    let mut candidates = pending.iter().enumerate().filter(|(_, intent)| {
        intent.correlated && intent.client_user_id.as_deref() == client_user_id
    });
    let candidate = candidates.next()?;
    candidates.next().is_none().then_some(candidate.0)
}

fn retain_terminal_intent(
    intent: &AgentEffortIntent,
    terminal_ids: &std::collections::HashSet<String>,
    client_user_id: Option<&str>,
) -> bool {
    !intent.correlated
        || !terminal_ids.contains(intent.tool_use_id.as_str())
        || client_user_id.is_some_and(|id| intent.client_user_id.as_deref() != Some(id))
}

fn agent_prompt<'a>(tool_name: &str, arguments: &'a Value) -> Option<&'a str> {
    is_agent_tool(tool_name)
        .then(|| arguments.get("prompt").and_then(Value::as_str))
        .flatten()
}
#[cfg(test)]
pub(super) fn prepare_arguments(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
) -> (Option<Value>, Value) {
    prepare_arguments_for_user(
        tool_name,
        tool_use_id,
        arguments,
        &[],
        &serde_json::json!(null),
    )
}

pub(super) fn prepare_arguments_for_user(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
    user_messages: &[Value],
    _system: &Value,
) -> (Option<Value>, Value) {
    let mut correlated = arguments.clone();
    let Some(prompt) = agent_prompt(tool_name, arguments) else {
        let mut public = correlated.clone();
        return (None, sanitize_public_tool_arguments(tool_name, &mut public));
    };
    super::agent_routing::hydrate_routing_fields(&mut correlated);
    correlated["prompt"] = Value::String(super::agent_effort_matching::correlated_prompt(
        prompt,
        tool_use_id,
        requested_model(arguments),
    ));
    let mut public_arguments = correlated.clone();
    let mut claude_arguments = sanitize_public_tool_arguments(tool_name, &mut public_arguments);
    let public = claude_arguments
        .as_object_mut()
        .expect("Agent arguments must be an object");
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

fn sanitize_public_tool_arguments(tool_name: &str, arguments: &mut Value) -> Value {
    let Some(public) = arguments.as_object_mut() else {
        return arguments.clone();
    };
    public.remove(ADAPTER_EFFORT);
    public.remove(ADAPTER_MODEL);
    public.remove(IMPLICIT_MODEL);
    if is_agent_tool(tool_name) {
        public.remove("model");
        public
            .entry("run_in_background".to_owned())
            .or_insert(Value::Bool(true));
    }
    arguments.clone()
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

#[cfg(test)]
fn tool_schema(_tool_name: &str, schema: Value) -> Value {
    schema
}

fn normalized_effort(value: &str) -> Option<&str> {
    let normalized = if value == "mid" { "medium" } else { value };
    valid_effort(normalized).then_some(normalized)
}

#[cfg(test)]
include!("agent_effort_tests.rs");
