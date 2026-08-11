use std::{collections::HashMap, path::PathBuf, sync::Mutex};

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
}

#[path = "subagent_reuse_ops.rs"]
mod ops;

#[path = "subagent_reuse_launch.rs"]
mod launch;

#[cfg(test)]
#[path = "subagent_reuse_tests.rs"]
mod tests;
