use super::{Event, LastNotify, NotifyKind};

#[cfg(test)]
pub(in crate::launcher) fn should_emit(event: &Event, last: Option<&LastNotify>) -> bool {
    should_emit_at(event, last, now_unix())
}

pub(in crate::launcher) fn should_emit_at(
    event: &Event,
    last: Option<&LastNotify>,
    now_unix: u64,
) -> bool {
    let _ = now_unix;
    // Waiting/Live were notifying alongside Complete for the same build_id
    // (three banners per install). Only swap-complete is user-facing now.
    if event.kind() != NotifyKind::Complete {
        return false;
    }
    let Some(last) = last else {
        return true;
    };
    if last.build_id == event.build_id() {
        // One Complete per build. If an older binary recorded Waiting/Live only,
        // still allow the eventual Complete once.
        return last.kind != NotifyKind::Complete;
    }
    // A newer build always gets its own Complete. Loop/hot-swap bursts used to
    // share a 5-minute quiet window and dropped real "差し替え完了" banners.
    true
}

pub(in crate::launcher) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
