use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use super::super::agent_effort::{AgentEffortIntent, MAX_PENDING_INTENTS};
use super::super::subscription::valid_effort;
use super::{MAX_AGE_SECONDS, StoredAgentIntent, unix_seconds};

pub(super) fn bound_intents(
    mut intents: VecDeque<StoredAgentIntent>,
) -> VecDeque<StoredAgentIntent> {
    while intents.len() > MAX_PENDING_INTENTS {
        intents.pop_front();
    }
    intents
}

pub(super) fn bound_vec(mut intents: Vec<StoredAgentIntent>) -> Vec<StoredAgentIntent> {
    let excess = intents.len().saturating_sub(MAX_PENDING_INTENTS);
    if excess > 0 {
        intents.drain(..excess);
    }
    intents
}

pub(super) fn valid_stored_intent(intent: &StoredAgentIntent) -> bool {
    !intent.tool_use_id.is_empty()
        && intent.effort.as_deref().is_none_or(valid_effort)
        && intent
            .model_override
            .as_deref()
            .is_none_or(|model| !model.is_empty())
}

pub(super) fn is_fresh(intent: &StoredAgentIntent) -> bool {
    unix_seconds().saturating_sub(intent.created_unix_seconds) <= MAX_AGE_SECONDS
}

pub(super) fn cache_read_failure(
    path: &Path,
    error: std::io::Error,
) -> VecDeque<StoredAgentIntent> {
    if error.kind() != std::io::ErrorKind::NotFound {
        tracing::warn!(%error, path = %path.display(), "could not restore persisted Agent intents");
    }
    VecDeque::new()
}

pub(super) fn restored_intent(stored: StoredAgentIntent) -> AgentEffortIntent {
    AgentEffortIntent {
        client_user_id: stored.client_user_id,
        prompt: String::new(),
        correlated: true,
        effort: stored.effort,
        model_override: stored.model_override,
        model_is_inherited: stored.model_is_inherited,
        run_in_background: stored.run_in_background,
        tool_use_id: stored.tool_use_id,
        created_at: std::time::Instant::now(),
        created_unix_seconds: stored.created_unix_seconds,
    }
}

pub(super) fn stored_intent(intent: &AgentEffortIntent) -> StoredAgentIntent {
    StoredAgentIntent {
        client_user_id: intent.client_user_id.clone(),
        effort: intent.effort.clone(),
        model_override: intent.model_override.clone(),
        model_is_inherited: intent.model_is_inherited,
        run_in_background: intent.run_in_background,
        tool_use_id: intent.tool_use_id.clone(),
        created_unix_seconds: intent.created_unix_seconds,
    }
}

pub(super) fn parent_directory(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

pub(super) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(super) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}
