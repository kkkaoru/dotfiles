#![allow(clippy::excessive_nesting)]

use std::{
    collections::HashMap,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{ClaimRecord, ClaimRequest, Store};

impl Store {
    pub(crate) fn acquire_claim(
        &self,
        request: ClaimRequest,
        now: u64,
    ) -> std::io::Result<Option<ClaimRecord>> {
        if request.session_id.is_empty() || request.scope.is_empty() || request.owner.is_empty() {
            return Ok(None);
        }
        self.with_locked_document(|document| {
            reap_claims(&mut document.claims, now);
            let key = claim_key(
                &request.session_id,
                &request.scope,
                request.model.as_deref(),
            );
            if let Some(existing) = document.claims.get(&key) {
                return Ok((existing.owner == request.owner).then(|| existing.clone()));
            }
            document.revision = document.revision.saturating_add(1);
            let claim = ClaimRecord {
                session_id: request.session_id,
                scope: request.scope,
                model: request.model,
                owner: request.owner,
                pid: request.pid,
                created_revision: document.revision,
                expires_unix_seconds: request.expires_unix_seconds,
                tool_use_id: request.tool_use_id,
            };
            document.claims.insert(key, claim.clone());
            Ok(Some(claim))
        })
    }

    pub(crate) fn release_claim(&self, claim: &ClaimRecord, now: u64) -> std::io::Result<bool> {
        self.with_locked_document(|document| {
            reap_claims(&mut document.claims, now);
            let key = claim_key(&claim.session_id, &claim.scope, claim.model.as_deref());
            let owned = document.claims.get(&key).is_some_and(|current| {
                current.owner == claim.owner && current.created_revision == claim.created_revision
            });
            if !owned {
                return Ok(false);
            }
            document.claims.remove(&key);
            document.revision = document.revision.saturating_add(1);
            Ok(true)
        })
    }

    pub(crate) fn claims_occupy(
        &self,
        session_id: &str,
        scope: &str,
        model: Option<&str>,
        now: u64,
    ) -> std::io::Result<bool> {
        self.with_locked_document(|document| {
            let before = document.claims.len();
            reap_claims(&mut document.claims, now);
            let occupied = document.claims.values().any(|claim| {
                claim.session_id == session_id
                    && super::occupancy_matches(&claim.scope, claim.model.as_deref(), scope, model)
            });
            if document.claims.len() != before {
                document.revision = document.revision.saturating_add(1);
            }
            Ok(occupied)
        })
    }

    pub(crate) fn session_state(&self, session_id: &str) -> Option<super::SessionState> {
        self.read_document()
            .and_then(|document| document.sessions.get(session_id).cloned())
    }
}

fn claim_key(session_id: &str, scope: &str, model: Option<&str>) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        session_id,
        normalize_scope(scope),
        model.map(str::to_ascii_lowercase).unwrap_or_default()
    )
}

pub(crate) fn normalize_scope(scope: &str) -> String {
    scope
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn reap_claims(claims: &mut HashMap<String, ClaimRecord>, now: u64) {
    claims.retain(|_, claim| claim_is_live(claim, now));
}

fn claim_is_live(claim: &ClaimRecord, now: u64) -> bool {
    claim.expires_unix_seconds > now && process_is_alive(claim.pid)
}

fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) is an existence probe and sends no signal.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(crate) fn current_pid() -> u32 {
    process::id()
}
