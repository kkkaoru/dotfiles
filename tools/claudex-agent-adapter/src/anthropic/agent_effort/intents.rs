use std::time::Instant;

use serde_json::Value;

use super::{
    ADAPTER_EFFORT, AgentEffort, AgentEffortIntent, AgentEffortIntents, AgentIntent, IMPLICIT_MODEL,
    MAX_PENDING_INTENTS, agent_prompt, is_agent_tool, is_subagent_request, normalized_effort,
    requested_model,
};
use super::background_launch;
use super::terminal::terminal_task_notification_ids;
use super::super::{
    AgentEffortRecord, MessagesRequest,
    agent_effort_matching::{has_correlation_marker, request_matches_intent_with_system},
    agent_intent_store::{persistence_snapshot, remove_expired, unix_seconds},
};

#[path = "intents_helpers.rs"]
mod intents_helpers;
use intents_helpers::{authorized_model, unique_correlated_candidate};
#[cfg(test)]
pub(super) use intents_helpers::retain_terminal_intent;

impl AgentEffortIntents {
    pub(in crate::anthropic) fn record_from_user_messages(
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
        // Match public Claude Code args: Agent/Task stay background unless the
        // active user explicitly required a synchronous result.
        let run_in_background = if is_agent_tool(tool_name) {
            background_launch::agent_launch_is_background(tool_name, user_messages)
        } else {
            arguments
                .get("run_in_background")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
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
    pub(in crate::anthropic) fn take(&self, request: &MessagesRequest) -> AgentIntent {
        if !is_subagent_request(request) {
            return AgentIntent::unmatched(false);
        }
        let client_user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        let matches = pending
            .iter()
            .enumerate()
            .filter(|(_, intent)| {
                request_matches_intent_with_system(&request.system, &request.messages, intent)
                    && (intent.correlated || intent.client_user_id.as_deref() == client_user_id)
            })
            .map(|(index, intent)| (index, intent.correlated))
            .collect::<Vec<_>>();
        // Correlated markers can coexist after compaction; prefer the newest.
        // Uncorrelated identical prompts stay FIFO so parallel launches dequeue
        // in launch order.
        let index = matches
            .iter()
            .rev()
            .find(|(_, correlated)| *correlated)
            .map(|(index, _)| *index)
            .or_else(|| matches.first().map(|(index, _)| *index))
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
}

#[path = "intents_retire.rs"]
mod retire;
