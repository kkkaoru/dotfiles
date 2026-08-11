use std::{fs, net::SocketAddr, path::Path, process::ExitStatus};

use anyhow::{Context, Result};

use super::super::{launcher_lock, launcher_logs, macos_notify_dispatch};
#[cfg(not(test))]
use super::script::osascript_command;
#[cfg(test)]
use super::{EVENTS, TEST_SPAWN, synthetic_success};
use super::{
    Event, LastNotify, Notification, now_unix,
    script::{deliver_status, notification},
    should_emit_at,
};

pub(in crate::launcher) fn post(cache: &Path, listen: &SocketAddr, event: Event) {
    if !macos_notify_dispatch::notifications_enabled() {
        return;
    }
    macos_notify_dispatch::post(cache, listen, event);
}

pub(in crate::launcher) fn post_in_process(cache: &Path, listen: &SocketAddr, event: Event) {
    if !macos_notify_dispatch::notifications_enabled() {
        return;
    }
    let lock_path = launcher_logs::hot_swap_notify_lock_path(cache);
    let _lock = match launcher_lock::acquire(&lock_path) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("claudex: macOS notification lock failed ({error:#})");
            return;
        }
    };
    let now = now_unix();
    let previous = read_last(cache, listen);
    if !should_emit_at(&event, previous.as_ref(), now) {
        // Do not slide emitted_unix on suppress: that extended quiet windows and
        // also made it look like a second Complete had been recorded.
        return;
    }
    record_event(&event);
    let mut last = LastNotify::from(&event);
    last.emitted_unix = now;
    if let Err(error) = write_last(cache, listen, &last) {
        eprintln!("claudex: macOS notification dedupe state failed ({error:#})");
    }
    let notification = notification(&event);
    if let Err(error) = deliver(&notification) {
        eprintln!("claudex: macOS notification failed ({error:#})");
    }
}

pub(in crate::launcher) fn deliver(notification: &Notification) -> Result<()> {
    let status = spawn_notification(notification).context("start osascript")?;
    deliver_status(status)
}

pub(in crate::launcher) fn read_last(cache: &Path, listen: &SocketAddr) -> Option<LastNotify> {
    let bytes = fs::read(launcher_logs::hot_swap_notify_path(cache, listen)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(super) fn write_last(cache: &Path, listen: &SocketAddr, last: &LastNotify) -> Result<()> {
    fs::create_dir_all(cache).context("create macOS notification cache")?;
    fs::write(
        launcher_logs::hot_swap_notify_path(cache, listen),
        serde_json::to_vec(last).context("encode macOS notification dedup state")?,
    )
    .context("write macOS notification dedup state")
}

pub(super) fn spawn_notification(notification: &Notification) -> std::io::Result<ExitStatus> {
    #[cfg(test)]
    {
        if let Some(spawn) = TEST_SPAWN.with(std::cell::Cell::get) {
            return spawn(notification);
        }
        let _ = notification;
        Ok(synthetic_success())
    }
    #[cfg(not(test))]
    osascript_command(notification).status()
}

pub(super) fn record_event(event: &Event) {
    #[cfg(test)]
    EVENTS.with(|events| events.borrow_mut().push(event.clone()));
    let _ = event;
}
