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
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{AgentEffort, AgentEffortIntents, prepare_arguments, tool_schema};
    use crate::anthropic::MessagesRequest;

    fn request(user_id: &str, prompt: &str, subagent: bool) -> MessagesRequest {
        let marker = if subagent { "cc_is_subagent=true" } else { "" };
        MessagesRequest {
            model: "resolved-model".to_owned(),
            system: json!([{"type":"text","text":marker}]),
            messages: vec![json!({
                "role":"user", "content":[{"type":"text","text":prompt}]
            })],
            tools: Vec::new(),
            stream: false,
            output_config: json!({"effort":"low"}),
            metadata: json!({"user_id":user_id}),
            claudex_collaborator_model: None,
        }
    }

    fn explicit(effort: AgentEffort) -> String {
        match effort {
            AgentEffort::Explicit(value) => value,
            AgentEffort::Unmatched | AgentEffort::ConfiguredDefault => {
                panic!("expected explicit Agent effort")
            }
        }
    }

    #[test]
    fn correlates_explicit_effort_by_client_session_and_prompt() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a".to_owned(),
            &json!({"prompt":"task-a","effort":"high"}),
        );
        assert!(matches!(
            intents.take(&request("session-a", "task-a", false)),
            AgentEffort::Unmatched
        ));
        assert_eq!(
            explicit(intents.take(&request("session-a", "task-a", true))),
            "high"
        );
    }

    #[test]
    fn correlates_parallel_and_repeated_prompts_without_crossing_sessions() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a1".to_owned(),
            &json!({"prompt":"same","effort":"high"}),
        );
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a2".to_owned(),
            &json!({"prompt":"same","effort":"low"}),
        );
        intents.record(
            Some("session-b"),
            "Agent",
            "tool-b".to_owned(),
            &json!({"prompt":"same","effort":"medium"}),
        );
        assert_eq!(
            explicit(intents.take(&request("session-b", "same", true))),
            "medium"
        );
        assert_eq!(
            explicit(intents.take(&request("session-a", "same", true))),
            "high"
        );
        assert_eq!(
            explicit(intents.take(&request("session-a", "same", true))),
            "low"
        );
    }

    #[test]
    fn unique_markers_correlate_reversed_identical_prompt_launches() {
        let intents = AgentEffortIntents::default();
        let (first, _) = prepare_arguments(
            "Agent",
            "tool-first",
            &json!({"prompt":"same","effort":"high"}),
        );
        let (second, _) = prepare_arguments(
            "Agent",
            "tool-second",
            &json!({"prompt":"same","effort":"low"}),
        );
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-first".to_owned(),
            first.as_ref().expect("first intent"),
        );
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-second".to_owned(),
            second.as_ref().expect("second intent"),
        );
        let first = first.expect("first intent");
        let second = second.expect("second intent");
        let second_prompt = second["prompt"].as_str().expect("second prompt");
        let first_prompt = first["prompt"].as_str().expect("first prompt");
        assert_eq!(
            explicit(intents.take(&request_without_user_id(second_prompt))),
            "low"
        );
        assert_eq!(
            explicit(intents.take(&request_without_user_id(first_prompt))),
            "high"
        );
    }

    #[test]
    fn an_agent_without_explicit_effort_uses_configured_default() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session"),
            "Agent",
            "tool".to_owned(),
            &json!({"prompt":"task"}),
        );
        assert!(matches!(
            intents.take(&request("session", "task", true)),
            AgentEffort::ConfiguredDefault
        ));
    }

    #[test]
    fn adds_and_strips_adapter_only_agent_effort() {
        let schema = tool_schema("Agent", json!({"type":"object"}));
        assert_eq!(
            schema["properties"]["claudex_effort"]["enum"],
            json!(["low", "medium", "high", "xhigh", "max"])
        );
        let (internal, public) = prepare_arguments(
            "Agent",
            "tool-mid",
            &json!({"prompt":"task","claudex_effort":"mid"}),
        );
        let internal = internal.expect("agent intent");
        assert_eq!(internal["claudex_effort"], "mid");
        assert!(public.get("claudex_effort").is_none());

        let intents = AgentEffortIntents::default();
        intents.record(None, "Agent", "tool-mid".to_owned(), &internal);
        assert_eq!(
            explicit(intents.take(&request_without_user_id(
                internal["prompt"].as_str().expect("correlated prompt")
            ))),
            "medium"
        );
    }

    #[test]
    fn preserves_native_effort_and_non_agent_schemas() {
        let (_, public) =
            prepare_arguments("Agent", "tool", &json!({"prompt":"task","effort":"high"}));
        assert_eq!(public["effort"], "high");
        assert_eq!(
            tool_schema("Read", json!({"type":"object"})),
            json!({"type":"object"})
        );
    }

    #[test]
    fn rejects_non_agents_invalid_efforts_and_unmatched_requests() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session"),
            "Read",
            "read".to_owned(),
            &json!({"prompt":"ignored"}),
        );
        intents.record(
            Some("session"),
            "Agent",
            "invalid".to_owned(),
            &json!({"prompt":"task","claudex_effort":"invalid"}),
        );
        assert!(matches!(
            intents.take(&request("other", "different", true)),
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request("session", "task", true)),
            AgentEffort::ConfiguredDefault
        ));
        let (internal, public) = prepare_arguments("Read", "read", &json!({"path":"file"}));
        assert!(internal.is_none());
        assert_eq!(public, json!({"path":"file"}));
    }

    #[test]
    fn bounds_pending_intents_and_removes_completed_tools() {
        let intents = AgentEffortIntents::default();
        for index in 0..=super::MAX_PENDING_INTENTS {
            intents.record(
                Some("session"),
                "Agent",
                format!("tool-{index}"),
                &json!({"prompt":format!("task-{index}")}),
            );
        }
        assert!(matches!(
            intents.take(&request("session", "task-0", true)),
            AgentEffort::Unmatched
        ));
        intents.remove_tool_results(["tool-1", "missing"].into_iter());
        assert!(matches!(
            intents.take(&request("session", "task-1", true)),
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request("session", "task-2", true)),
            AgentEffort::ConfiguredDefault
        ));
    }

    #[test]
    fn tolerates_invalid_and_preconfigured_agent_schemas() {
        assert_eq!(tool_schema("Agent", json!(null)), json!(null));
        assert_eq!(
            tool_schema("Agent", json!({"properties":"invalid"})),
            json!({"properties":"invalid"})
        );
        let existing = json!({
            "properties":{"claudex_effort":{"type":"string","const":"high"}}
        });
        assert_eq!(tool_schema("Agent", existing.clone()), existing);
    }

    fn request_without_user_id(prompt: &str) -> MessagesRequest {
        let mut request = request("ignored", prompt, true);
        request.metadata = json!({});
        request
    }
}
