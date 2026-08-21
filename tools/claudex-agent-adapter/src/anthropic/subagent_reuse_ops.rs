use serde_json::Value;

use super::records::{
    LaunchRecord, launch_model, launch_scope_key, merge_launches, occupancy_matches,
    terminal_status, unique_live_agent_count,
};
use super::{
    MessagesRequest, QueuedFollowUp, SubagentReuseRegistry, agent_teams_enabled,
    append_reuse_guidance, apply_transcript, explicit_follow_up_recipient, find_reusable_launch,
    follow_up_message, has_send_message_tool, is_send_message_follow_up, live_agent_task_ids,
    max_subagents_per_session, reuse_enabled, reuse_recipients, send_message_follow_up_arguments,
    session_id, set_limit_metadata, summarize_scope, system_contains_marker,
};

const QUEUED_FOLLOW_UP_SEPARATOR: &str = "\n\n";

impl SubagentReuseRegistry {
    pub(in crate::anthropic) fn observe_and_restore(&self, request: &mut MessagesRequest) {
        self.observe_and_restore_with_reuse(request, reuse_enabled());
    }

    pub(super) fn observe_and_restore_with_reuse(
        &self,
        request: &mut MessagesRequest,
        reuse: bool,
    ) {
        let Some(session_id) = session_id(request) else {
            return;
        };
        let mut states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let state = states.entry(session_id.clone()).or_default();
        let previous_launches = state.launches.clone(); // Avoid fsync when the transcript is unchanged.
        // Chronological: a later resume launch result must win over an earlier
        // completion notification still present in the transcript.
        apply_transcript(&mut state.launches, &request.messages);
        let limit_reached = unique_live_agent_count(&state.launches, &request.messages)
            >= max_subagents_per_session();
        set_limit_metadata(request, limit_reached);
        // Restore even when the transcript still lists launches: system may be
        // rebuilt without the marker while messages keep prior agentIds.
        let should_restore =
            reuse && !state.launches.is_empty() && !system_contains_marker(&request.system);
        let teams = agent_teams_enabled(request) && has_send_message_tool(&request.tools);
        let recipients =
            should_restore.then(|| reuse_recipients(&state.launches, &request.messages));
        let launches_changed = state.launches != previous_launches;
        let current_state = state.clone();
        drop(states);
        if launches_changed {
            self.persist_session(&session_id, current_state.clone());
        }
        self.resolve_claims(&session_id, &current_state.launches);
        self.shadow
            .observe_request(&session_id, request, &current_state.launches);
        if let Some(recipients) = recipients {
            append_reuse_guidance(&mut request.system, &recipients, teams);
        }
    }

    pub(in crate::anthropic) fn rewrite_launch_input(
        &self,
        session_id: &str,
        arguments: &mut Value,
    ) -> Option<String> {
        self.rewrite_launch_input_with_reuse(session_id, arguments, reuse_enabled())
    }

    pub(super) fn rewrite_launch_input_with_reuse(
        &self,
        session_id: &str,
        arguments: &mut Value,
        reuse: bool,
    ) -> Option<String> {
        self.observe_shadow_decision(session_id, arguments);
        if let Some(recipient) = convert_explicit_follow_up(arguments) {
            tracing::info!(
                session_id,
                recipient,
                "rewrote SubAgent launch into SendMessage follow-up"
            );
            return Some(recipient);
        }
        if explicit_follow_up_recipient(arguments).is_some() {
            return None;
        }
        if !reuse {
            return None;
        }
        self.reuse_same_scope_launch(session_id, arguments)
            .or_else(|| self.rewrite_path_colliding_writer(session_id, arguments))
    }

    pub(in crate::anthropic) fn occupied_recipient(
        &self,
        session_id: &str,
        arguments: &Value,
    ) -> Option<String> {
        self.live_occupant(session_id, arguments)
            .map(|launch| launch.recipient)
            .filter(|recipient| !recipient.is_empty())
    }

    pub(in crate::anthropic) fn queue_inflight_follow_up(
        &self,
        session_id: &str,
        arguments: &Value,
    ) -> bool {
        if session_id.is_empty() || is_send_message_follow_up(arguments) {
            return false;
        }
        if self.occupied_recipient(session_id, arguments).is_some() {
            return false;
        }
        if !self.scope_is_occupied(session_id, arguments) {
            return false;
        }
        let Some(message) = follow_up_message(arguments) else {
            return false;
        };
        let scope = summarize_scope(arguments);
        if scope.is_empty() {
            return false;
        }
        let model = launch_model(arguments).map(str::to_owned);
        let proposed = launch_scope_key(arguments);
        let mut queued = self
            .queued_follow_ups
            .lock()
            .expect("SubAgent follow-up queue poisoned");
        if queued.iter().any(|item| {
            item.session_id == session_id
                && item.message == message
                && occupancy_matches(
                    &item.scope,
                    item.model.as_deref(),
                    &proposed,
                    model.as_deref(),
                )
        }) {
            return true;
        }
        queued.push(QueuedFollowUp {
            session_id: session_id.to_owned(),
            scope,
            model,
            message,
        });
        true
    }

    fn rewrite_path_colliding_writer(
        &self,
        session_id: &str,
        arguments: &mut Value,
    ) -> Option<String> {
        if session_id.is_empty() || is_send_message_follow_up(arguments) {
            return None;
        }
        let incoming_scope = summarize_scope(arguments);
        if incoming_scope.is_empty() {
            return None;
        }
        let message = follow_up_message(arguments)?;
        let model = launch_model(arguments);
        let states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let occupant = states
            .get(session_id)?
            .launches
            .iter()
            .find(|launch| colliding_live_writer(launch, &incoming_scope, model))?;
        let recipient = occupant.recipient.clone();
        drop(states);
        let message =
            join_follow_up_messages(self.drain_queued_follow_ups(session_id, arguments), message);
        *arguments = send_message_follow_up_arguments(&recipient, &message, None);
        tracing::info!(
            session_id,
            recipient,
            "rewrote path-colliding SubAgent launch into SendMessage follow-up"
        );
        Some(recipient)
    }

    fn reuse_same_scope_launch(&self, session_id: &str, arguments: &mut Value) -> Option<String> {
        if session_id.is_empty() {
            return None;
        }
        let message = follow_up_message(arguments)?;
        let launches = self.session_launches(session_id);
        let recipient = find_reusable_launch(&launches, arguments)
            .map(|launch| launch.recipient.clone())
            .filter(|recipient| !recipient.is_empty())
            .or_else(|| self.occupied_recipient(session_id, arguments))?;
        let message =
            join_follow_up_messages(self.drain_queued_follow_ups(session_id, arguments), message);
        *arguments = send_message_follow_up_arguments(&recipient, &message, None);
        tracing::info!(
            session_id,
            recipient,
            "rewrote SubAgent launch into SendMessage follow-up"
        );
        Some(recipient)
    }

    fn session_launches(&self, session_id: &str) -> Vec<LaunchRecord> {
        let mut launches = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session_id)
            .map(|state| state.launches.clone())
            .unwrap_or_default();
        if let Some(state) = self
            .store
            .as_ref()
            .and_then(|store| store.session_state(session_id))
        {
            merge_launches(&mut launches, state.launches.iter());
        }
        launches
    }

    fn live_occupant(&self, session_id: &str, arguments: &Value) -> Option<LaunchRecord> {
        if session_id.is_empty() {
            return None;
        }
        let proposed = launch_scope_key(arguments);
        if proposed.is_empty() {
            return None;
        }
        let model = launch_model(arguments);
        self.session_launches(session_id)
            .into_iter()
            .rev()
            .find(|launch| {
                !terminal_status(&launch.status)
                    && occupancy_matches(&launch.scope, launch.model.as_deref(), &proposed, model)
            })
    }

    fn drain_queued_follow_ups(&self, session_id: &str, arguments: &Value) -> Vec<String> {
        let proposed = launch_scope_key(arguments);
        let model = launch_model(arguments);
        let mut queued = self
            .queued_follow_ups
            .lock()
            .expect("SubAgent follow-up queue poisoned");
        let (taken, kept) = queued.drain(..).partition(|item| {
            item.session_id == session_id
                && occupancy_matches(&item.scope, item.model.as_deref(), &proposed, model)
        });
        *queued = kept;
        taken.into_iter().map(|item| item.message).collect()
    }

    pub(in crate::anthropic) fn live_agent_count(
        &self,
        session_id: &str,
        messages: &[Value],
    ) -> usize {
        if session_id.is_empty() {
            return live_agent_task_ids(messages).len();
        }
        let launches = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session_id)
            .map(|state| state.launches.clone())
            .unwrap_or_default();
        unique_live_agent_count(&launches, messages)
    }

    pub(in crate::anthropic) fn session_at_live_capacity(
        &self,
        session_id: &str,
        messages: &[Value],
    ) -> bool {
        super::should_reject_live_cap(
            self.live_agent_count(session_id, messages),
            max_subagents_per_session(),
            &Value::Null,
        )
    }

    fn observe_shadow_decision(&self, session_id: &str, arguments: &Value) {
        if session_id.is_empty() {
            return;
        }
        let Ok(states) = self.states.lock() else {
            tracing::warn!(
                target: "claudex_subagent_reuse_shadow",
                "idle Phase 0 could not read the reuse registry"
            );
            return;
        };
        let Some(state) = states.get(session_id) else {
            return;
        };
        self.shadow
            .observe_decision(session_id, &state.launches, arguments);
    }
}

fn colliding_live_writer(
    launch: &LaunchRecord,
    incoming_scope: &str,
    incoming_model: Option<&str>,
) -> bool {
    !terminal_status(&launch.status)
        && !launch.recipient.is_empty()
        && occupancy_matches(
            &launch.scope,
            launch.model.as_deref(),
            incoming_scope,
            incoming_model,
        )
}

fn join_follow_up_messages(queued: Vec<String>, current: String) -> String {
    if queued.is_empty() {
        return current;
    }
    let mut parts = queued;
    parts.push(current);
    parts.join(QUEUED_FOLLOW_UP_SEPARATOR)
}

fn convert_explicit_follow_up(arguments: &mut Value) -> Option<String> {
    let recipient = explicit_follow_up_recipient(arguments)?;
    let message = follow_up_message(arguments)?;
    let summary = arguments
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    *arguments = send_message_follow_up_arguments(&recipient, &message, summary.as_deref());
    Some(recipient)
}
