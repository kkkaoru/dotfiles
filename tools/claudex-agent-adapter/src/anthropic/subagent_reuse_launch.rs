use std::collections::HashMap;

use serde_json::Value;

#[cfg(test)]
use super::MessagesRequest;
use super::{
    LaunchRecord, SessionState, SubagentReuseRegistry, launch_model, records, reuse_enabled,
    scope_is_occupied, summarize_scope,
};

impl SubagentReuseRegistry {
    pub(in crate::anthropic) fn scope_is_occupied(
        &self,
        session_id: &str,
        arguments: &Value,
    ) -> bool {
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
    pub(in crate::anthropic) fn note_inflight_launch(
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

    pub(super) fn persist(&self, states: HashMap<String, SessionState>) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(error) = store.save(states) {
            tracing::warn!(%error, path = %store.path.display(), "could not persist SubAgent reuse registry");
        }
    }
}
