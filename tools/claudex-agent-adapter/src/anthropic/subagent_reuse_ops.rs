use serde_json::{Value, json};

use super::{
    MessagesRequest, SubagentReuseRegistry, agent_teams_enabled, already_has_resume,
    append_reuse_guidance, apply_transcript, find_reusable_launch, has_send_message_tool,
    max_subagents_per_session, reuse_enabled, reuse_recipients, session_id, set_limit_metadata,
    system_contains_marker,
};

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
        let limit_reached = state.launches.len() >= max_subagents_per_session();
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
        if !reuse || session_id.is_empty() || already_has_resume(arguments) {
            return None;
        }
        let states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let launch = find_reusable_launch(&states.get(session_id)?.launches, arguments)?;
        // Skip resume injection if recipient is empty (pending or in-flight without confirmation)
        if launch.recipient.is_empty() {
            return None;
        }
        let recipient = launch.recipient.clone();
        drop(states);
        let object = arguments.as_object_mut()?;
        object.insert("resume".to_owned(), json!(recipient));
        tracing::info!(
            session_id,
            recipient,
            "rewrote SubAgent launch into resume of a compatible worker"
        );
        Some(recipient)
    }
}
