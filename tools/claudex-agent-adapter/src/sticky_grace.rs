use std::time::Duration;

/// Keep sticky / retained generations through interactive pauses so warm
/// SubAgent threads and prompt-cache prefixes survive between turns. Matches
/// adapter `IDLE_SESSION_TTL` (2h); the old 45s window released retained
/// daemons while the bridge still expected reuse.
pub(crate) const STICKY_IDLE_GRACE_SECS: u64 = 120 * 60;

pub(crate) const STICKY_IDLE_GRACE: Duration = Duration::from_secs(STICKY_IDLE_GRACE_SECS);

pub(crate) fn within_sticky_idle_grace_secs(idle_seconds: Option<u64>) -> bool {
    // Older adapters omit idle_seconds: fall through to immediate idle release.
    idle_seconds.is_some_and(|secs| secs <= STICKY_IDLE_GRACE_SECS)
}
