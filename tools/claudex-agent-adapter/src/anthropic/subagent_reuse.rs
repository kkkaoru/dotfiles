use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use super::MessagesRequest;
mod guidance;
mod limits;
mod records;
mod records_scope;
mod records_status;
mod store;
#[cfg(test)]
use guidance::REUSE_GUIDANCE_MARKER;
pub(super) use guidance::{agent_teams_enabled, value_text};
use guidance::{append_reuse_guidance, has_send_message_tool, system_contains_marker};
pub(super) use limits::{
    is_launch_tool, max_subagents_per_session, reuse_enabled, session_id,
    should_expose_launch_tools,
};
pub(in crate::anthropic) use records::live_agent_task_ids;
use records::{
    LaunchRecord, already_has_resume, apply_transcript, find_reusable_launch, launch_model,
    scope_is_occupied, summarize_scope,
};
use store::{
    CACHE_FILE_NAME, ClaimRecord, SessionState, Store, reuse_recipients, set_limit_metadata,
};

pub(crate) const MAX_SUBAGENTS_PER_SESSION_ENV: &str = "CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION";
pub(crate) const DEFAULT_MAX_SUBAGENTS_PER_SESSION: usize = 1_024;

pub(super) struct SubagentReuseRegistry {
    states: Mutex<HashMap<String, SessionState>>,
    session_revisions: Mutex<HashMap<String, u64>>,
    claims: Mutex<HashMap<String, ClaimRecord>>,
    store: Option<Store>,
    owner: String,
}

impl Default for SubagentReuseRegistry {
    fn default() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
            session_revisions: Mutex::new(HashMap::new()),
            claims: Mutex::new(HashMap::new()),
            store: None,
            owner: owner_token(),
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
        let loaded = store.load_snapshot();
        Self {
            states: Mutex::new(loaded.sessions),
            session_revisions: Mutex::new(loaded.session_revisions),
            claims: Mutex::new(HashMap::new()),
            store: Some(store),
            owner: owner_token(),
        }
    }

    #[cfg(test)]
    pub(super) fn with_store(path: PathBuf) -> Self {
        let store = Store::new(path);
        let loaded = store.load_snapshot();
        Self {
            states: Mutex::new(loaded.sessions),
            session_revisions: Mutex::new(loaded.session_revisions),
            claims: Mutex::new(HashMap::new()),
            store: Some(store),
            owner: owner_token(),
        }
    }
}

#[path = "subagent_reuse_ops.rs"]
mod ops;

#[path = "subagent_reuse_launch.rs"]
mod launch;

#[cfg(test)]
#[path = "subagent_reuse_tests.rs"]
mod tests;

fn owner_token() -> String {
    static NEXT_OWNER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
    )
}
