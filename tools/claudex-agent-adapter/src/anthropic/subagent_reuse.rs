use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use serde_json::{Value, json};

use super::MessagesRequest;
mod guidance;
mod records;
mod records_scope;
mod records_status;
mod store;
mod limits;
pub(super) use limits::{
    is_launch_tool, max_subagents_per_session, reuse_enabled, session_id,
    should_expose_launch_tools,
};
#[cfg(test)]
use guidance::REUSE_GUIDANCE_MARKER;
pub(super) use guidance::{agent_teams_enabled, value_text};
use guidance::{append_reuse_guidance, has_send_message_tool, system_contains_marker};
pub(in crate::anthropic) use records::live_agent_task_ids;
use records::{
    LaunchRecord, already_has_resume, apply_transcript, find_reusable_launch, launch_model,
    scope_is_occupied, summarize_scope,
};
#[cfg(test)]
use store::StoredStates;
use store::{
    CACHE_FILE_NAME, SessionState, Store, reuse_recipients,
    set_limit_metadata,
};

pub(crate) const MAX_SUBAGENTS_PER_SESSION_ENV: &str = "CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION";
pub(crate) const DEFAULT_MAX_SUBAGENTS_PER_SESSION: usize = 1_024;

pub(super) struct SubagentReuseRegistry {
    states: Mutex<HashMap<String, SessionState>>,
    store: Option<Store>,
}

impl Default for SubagentReuseRegistry {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            store: None,
        }
    }
}

impl SubagentReuseRegistry {
    pub(super) fn persistent() -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        let store = Store::new(
            PathBuf::from(home)
                .join(".cache/claudex")
                .join(CACHE_FILE_NAME),
        );
        Self {
            states: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    #[cfg(test)]
    pub(super) fn with_store(path: PathBuf) -> Self {
        let store = Store::new(path);
        Self {
            states: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    pub(super) fn observe_and_restore(&self, request: &mut MessagesRequest) {
        self.observe_and_restore_with_reuse(request, reuse_enabled());
    }

    fn observe_and_restore_with_reuse(&self, request: &mut MessagesRequest, reuse: bool) {
        let Some(session_id) = session_id(request) else {
            return;
        };
        let mut states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let state = states.entry(session_id).or_default();
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
        let snapshot = states.clone();
        drop(states);
        if launches_changed {
            self.persist(snapshot);
        }
        if let Some(recipients) = recipients {
            append_reuse_guidance(&mut request.system, &recipients, teams);
        }
    }

    pub(super) fn rewrite_launch_input(
        &self,
        session_id: &str,
        arguments: &mut Value,
    ) -> Option<String> {
        self.rewrite_launch_input_with_reuse(session_id, arguments, reuse_enabled())
    }

    fn rewrite_launch_input_with_reuse(
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

    pub(super) fn scope_is_occupied(&self, session_id: &str, arguments: &Value) -> bool {
        let scope_key = records::launch_scope_key(arguments);
        if session_id.is_empty() || scope_key.is_empty() {
            return false;
        }
        let model = launch_model(arguments);
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session_id)
            .is_some_and(|state| scope_is_occupied(&state.launches, &scope_key, model))
    }

    /// Remember a just-forwarded launch before its tool_result exists so a
    /// same-turn duplicate cannot spawn another same-model worker.
    pub(super) fn note_inflight_launch(
        &self,
        session_id: &str,
        arguments: &Value,
        tool_use_id: &str,
    ) {
        if !reuse_enabled() || session_id.is_empty() || tool_use_id.is_empty() {
            return;
        }
        let scope = summarize_scope(arguments);
        if scope.is_empty() {
            return;
        }
        let model = launch_model(arguments).map(str::to_owned);
        let mut states = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned");
        let state = states.entry(session_id.to_owned()).or_default();
        records::merge_launches(
            &mut state.launches,
            std::iter::once(&LaunchRecord {
                key: tool_use_id.to_owned(),
                recipient: String::new(),
                scope,
                model,
                status: "pending".to_owned(),
            }),
        );
    }

    #[cfg(test)]
    pub(super) fn observe_and_restore_for_test(&self, request: &mut MessagesRequest, reuse: bool) {
        self.observe_and_restore_with_reuse(request, reuse);
    }

    #[cfg(test)]
    pub(super) fn rewrite_launch_input_for_test(
        &self,
        session_id: &str,
        arguments: &mut Value,
        reuse: bool,
    ) -> Option<String> {
        self.rewrite_launch_input_with_reuse(session_id, arguments, reuse)
    }

    #[cfg(test)]
    pub(super) fn state_for(&self, session: &str) -> Option<Vec<String>> {
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session)
            .map(|state| {
                state
                    .launches
                    .iter()
                    .map(|launch| launch.recipient.clone())
                    .filter(|recipient| !recipient.is_empty())
                    .collect()
            })
    }

    #[cfg(test)]
    pub(super) fn status_for(&self, session: &str, recipient: &str) -> Option<String> {
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session)?
            .launches
            .iter()
            .find(|launch| launch.recipient == recipient)
            .map(|launch| launch.status.clone())
    }

    fn persist(&self, states: HashMap<String, SessionState>) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(error) = store.save(states) {
            tracing::warn!(%error, path = %store.path.display(), "could not persist SubAgent reuse registry");
        }
    }
}


#[cfg(test)]
#[path = "subagent_reuse_tests.rs"]
mod tests;
