//! Session-scoped delegation policy snapshots shared with the PreToolUse hook.

mod fs;
mod lock;
mod prompt;
mod publish;
#[cfg(test)]
mod tests;

pub use prompt::{effective_summary, session_id};
pub use publish::write_delegation_state_at;

pub const STATE_VERSION: u64 = 2;
pub const STATE_DIRECTORY: &str = "delegation-state-v2";
pub const MAX_SESSION_ID_BYTES: usize = 256;
pub const STATE_TTL_SECONDS: f64 = 86_400.0;
const MAX_STATE_BYTES: u64 = 16 * 1024;
const MAX_SELECTED_WORKERS: u64 = 256;
const MAX_FUTURE_SKEW_SECONDS: f64 = 300.0;
const LEGACY_STATE_FILE: &str = "delegation-state.json";
const STATE_KEYS: &[&str] = &[
    "version",
    "session_key",
    "updated_at",
    "expires_at",
    "base_delegation_required",
    "prompt_opt_out",
    "delegation_required",
    "selected_workers_count",
    "direct_main_execution",
];
