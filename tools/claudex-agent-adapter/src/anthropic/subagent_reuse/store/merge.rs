#![allow(clippy::collapsible_if, clippy::excessive_nesting)]

use super::{SessionState, StoredStates};
use crate::anthropic::subagent_reuse::records::merge_launches;

const MAX_PERSISTED_RECIPIENTS: usize = 1_024;
const MAX_PERSISTED_CLAIMS: usize = 4_096;

pub(super) fn merge_session_state(current: &mut SessionState, incoming: &SessionState) {
    let before = current.launches.clone();
    merge_launches(&mut current.launches, incoming.launches.iter());
    for old in before {
        if let Some(found) = current
            .launches
            .iter_mut()
            .find(|launch| launch.key == old.key && !old.key.is_empty())
        {
            if is_terminal(&old.status) && !is_terminal(&found.status) {
                found.status = old.status;
            }
        }
    }
}

pub(super) fn prune_persisted_state(state: &mut SessionState) {
    let excess = state
        .launches
        .len()
        .saturating_sub(MAX_PERSISTED_RECIPIENTS);
    state.launches.drain(..excess);
}

pub(super) fn bound_document(document: &mut StoredStates) {
    document
        .sessions
        .retain(|session, _| !session.is_empty() && !document.tombstones.contains_key(session));
    document
        .sessions
        .values_mut()
        .for_each(prune_persisted_state);
    document
        .session_revisions
        .retain(|session, _| !session.is_empty());
    document.tombstones.retain(|session, _| !session.is_empty());
    if document.tombstones.len() > MAX_PERSISTED_RECIPIENTS {
        let mut entries = document
            .tombstones
            .iter()
            .map(|(key, revision)| (key.clone(), *revision))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(_, revision)| *revision);
        for (session, _) in entries
            .into_iter()
            .take(document.tombstones.len() - MAX_PERSISTED_RECIPIENTS)
        {
            document.tombstones.remove(&session);
        }
    }
    if document.claims.len() > MAX_PERSISTED_CLAIMS {
        let mut entries = document.claims.keys().cloned().collect::<Vec<_>>();
        entries.sort();
        for key in entries.into_iter().skip(MAX_PERSISTED_CLAIMS) {
            document.claims.remove(&key);
        }
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "canceled" | "timeout" | "stopped"
    )
}
