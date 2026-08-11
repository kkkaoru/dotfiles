use std::time::Duration;

/// Keep sticky / retained generations through brief idle gaps between turns.
pub(crate) const STICKY_IDLE_GRACE_SECS: u64 = 45;

pub(crate) const STICKY_IDLE_GRACE: Duration = Duration::from_secs(STICKY_IDLE_GRACE_SECS);

pub(crate) fn within_sticky_idle_grace_secs(idle_seconds: Option<u64>) -> bool {
    // Older adapters omit idle_seconds: fall through to immediate idle release.
    idle_seconds.is_some_and(|secs| secs <= STICKY_IDLE_GRACE_SECS)
}
