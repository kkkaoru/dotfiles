#![allow(clippy::excessive_nesting)]

#[cfg(test)]
use std::collections::HashMap;

use serde_json::Value;

#[cfg(test)]
use super::MessagesRequest;
use super::store::{
    CLAIM_TTL_SECONDS, ClaimRecord, ClaimRequest, current_pid, normalize_scope, unix_seconds,
};
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
        let local = self
            .states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .get(session_id)
            .is_some_and(|state| scope_is_occupied(&state.launches, &scope_key, model));
        if local {
            return true;
        }
        if let Some(store) = &self.store {
            let canonical = store
                .session_state(session_id)
                .is_some_and(|state| scope_is_occupied(&state.launches, &scope_key, model));
            let claimed = store
                .claims_occupy(session_id, &scope_key, model, unix_seconds())
                .unwrap_or(false);
            return canonical || claimed;
        }
        let now = unix_seconds();
        self.claims
            .lock()
            .expect("SubAgent claim registry poisoned")
            .values()
            .any(|claim| {
                claim.expires_unix_seconds > now
                    && claim.session_id == session_id
                    && normalize_scope(&claim.scope) == scope_key
                    && models_overlap(model, claim.model.as_deref())
            })
    }

    /// Remember a just-forwarded launch before its tool_result exists so a
    /// same-turn duplicate cannot spawn another same-model worker.
    pub(in crate::anthropic) fn note_inflight_launch(
        &self,
        session_id: &str,
        arguments: &Value,
        tool_use_id: &str,
    ) -> bool {
        if !reuse_enabled() || session_id.is_empty() || tool_use_id.is_empty() {
            return false;
        }
        let scope = summarize_scope(arguments);
        if scope.is_empty() {
            return false;
        }
        let model = launch_model(arguments).map(str::to_owned);
        let now = unix_seconds();
        let expires = now.saturating_add(CLAIM_TTL_SECONDS);
        let claim = if let Some(store) = &self.store {
            match store.acquire_claim(
                ClaimRequest {
                    session_id: session_id.to_owned(),
                    scope: scope.clone(),
                    model: model.clone(),
                    owner: self.owner.clone(),
                    pid: current_pid(),
                    tool_use_id: tool_use_id.to_owned(),
                    expires_unix_seconds: expires,
                },
                now,
            ) {
                Ok(claim) => claim,
                Err(error) => {
                    tracing::warn!(%error, session_id, "could not acquire SubAgent admission claim");
                    None
                }
            }
        } else {
            let mut claims = self
                .claims
                .lock()
                .expect("SubAgent claim registry poisoned");
            claims.retain(|_, claim| claim.expires_unix_seconds > now);
            if claims.values().any(|claim| {
                claim.session_id == session_id
                    && normalize_scope(&claim.scope) == normalize_scope(&scope)
                    && models_overlap(model.as_deref(), claim.model.as_deref())
            }) {
                None
            } else {
                let claim = ClaimRecord {
                    session_id: session_id.to_owned(),
                    scope: scope.clone(),
                    model: model.clone(),
                    owner: self.owner.clone(),
                    pid: current_pid(),
                    created_revision: claims.len() as u64 + 1,
                    expires_unix_seconds: expires,
                    tool_use_id: tool_use_id.to_owned(),
                };
                claims.insert(tool_use_id.to_owned(), claim.clone());
                Some(claim)
            }
        };
        let Some(claim) = claim else {
            return false;
        };
        self.claims
            .lock()
            .expect("SubAgent claim registry poisoned")
            .insert(tool_use_id.to_owned(), claim);
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
        true
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
    pub(super) fn forget_memory_for_test(&self) {
        self.states
            .lock()
            .expect("SubAgent reuse registry poisoned")
            .clear();
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

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn persist(&self, states: HashMap<String, SessionState>) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(error) = store.save(states) {
            tracing::warn!(%error, path = %store.path.display(), "could not persist SubAgent reuse registry");
        }
    }

    pub(super) fn persist_session(&self, session_id: &str, state: SessionState) {
        let Some(store) = &self.store else {
            return;
        };
        let base_revision = self
            .session_revisions
            .lock()
            .expect("SubAgent revision registry poisoned")
            .get(session_id)
            .copied()
            .unwrap_or_default();
        match store.save_session_delta(session_id, state, base_revision) {
            Ok(true) => {
                let revision = store
                    .load_snapshot()
                    .session_revisions
                    .get(session_id)
                    .copied()
                    .unwrap_or(base_revision);
                self.session_revisions
                    .lock()
                    .expect("SubAgent revision registry poisoned")
                    .insert(session_id.to_owned(), revision);
            }
            Ok(false) => {
                // A newer tombstone won the race; discard the stale in-memory
                // snapshot rather than letting the next turn rewrite it.
                if let Some(latest) = store.session_state(session_id) {
                    self.states
                        .lock()
                        .expect("SubAgent reuse registry poisoned")
                        .insert(session_id.to_owned(), latest);
                }
            }
            Err(error) => {
                tracing::warn!(%error, session_id, "could not persist SubAgent session delta")
            }
        }
    }

    pub(super) fn resolve_claims(&self, session_id: &str, launches: &[LaunchRecord]) {
        let handles = self
            .claims
            .lock()
            .expect("SubAgent claim registry poisoned")
            .iter()
            .filter(|(_, claim)| claim.session_id == session_id)
            .map(|(key, claim)| (key.clone(), claim.clone()))
            .collect::<Vec<_>>();
        for (key, claim) in handles {
            let resolved = launches.iter().any(|launch| {
                launch.key == claim.tool_use_id
                    && (!launch.recipient.is_empty() || records::terminal_status(&launch.status))
            });
            if !resolved {
                continue;
            }
            let released = self
                .store
                .as_ref()
                .map(|store| store.release_claim(&claim, unix_seconds()).unwrap_or(false))
                .unwrap_or_else(|| {
                    self.claims
                        .lock()
                        .expect("SubAgent claim registry poisoned")
                        .remove(&key)
                        .is_some()
                });
            if released {
                self.claims
                    .lock()
                    .expect("SubAgent claim registry poisoned")
                    .remove(&key);
            }
        }
    }
}

fn models_overlap(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, _) => true,
        (Some(_), None) => false,
    }
}
