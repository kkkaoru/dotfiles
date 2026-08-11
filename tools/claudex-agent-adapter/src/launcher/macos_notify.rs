use std::{
    fs,
    net::SocketAddr,
    path::Path,
    process::ExitStatus,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, launcher_lock, launcher_logs};

mod script;
use script::{deliver_status, notification, osascript_command};
#[cfg(test)]
use script::{escape_applescript, osascript_program};


/// One Complete banner per build_id. Waiting/Live stay silent (see should_emit).
pub(super) const TITLE: &str = "claudex";
pub(super) const WAITING_SUBTITLE: &str = "ビルド完了・待機中";
pub(super) const LIVE_SUBTITLE: &str = "live 更新完了";
pub(super) const COMPLETE_SUBTITLE: &str = "差し替え完了";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum NotifyKind {
    Waiting,
    Live,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct LastNotify {
    pub kind: NotifyKind,
    pub listen: String,
    pub build_id: String,
    /// Unix seconds of the last emit or suppressed rebuild attempt.
    /// Missing in older cache files → treated as 0 (cooldown inactive).
    #[serde(default)]
    pub emitted_unix: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Event {
    WaitingForIdle {
        listen: String,
        build_id: String,
        waiter_pid: u32,
    },
    LiveReady {
        listen: String,
        build_id: String,
        waiting: String,
    },
    SwapComplete {
        listen: String,
        build_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Notification {
    pub title: String,
    pub subtitle: String,
    pub body: String,
}

impl Event {
    pub(super) fn kind(&self) -> NotifyKind {
        match self {
            Self::WaitingForIdle { .. } => NotifyKind::Waiting,
            Self::LiveReady { .. } => NotifyKind::Live,
            Self::SwapComplete { .. } => NotifyKind::Complete,
        }
    }

    pub(super) fn listen(&self) -> &str {
        match self {
            Self::WaitingForIdle { listen, .. }
            | Self::LiveReady { listen, .. }
            | Self::SwapComplete { listen, .. } => listen,
        }
    }

    pub(super) fn build_id(&self) -> &str {
        match self {
            Self::WaitingForIdle { build_id, .. }
            | Self::LiveReady { build_id, .. }
            | Self::SwapComplete { build_id, .. } => build_id,
        }
    }
}

impl From<&Event> for LastNotify {
    fn from(event: &Event) -> Self {
        Self {
            kind: event.kind(),
            listen: event.listen().to_owned(),
            build_id: event.build_id().to_owned(),
            emitted_unix: 0,
        }
    }
}

pub(super) fn waiting_for_idle(config: &ServiceConfig, waiter_pid: u32) {
    let Some(cache) = config.log_path.parent() else {
        return;
    };
    post(
        cache,
        &config.options.listen,
        Event::WaitingForIdle {
            listen: config.options.listen.to_string(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            waiter_pid,
        },
    );
}

pub(super) fn live_ready(config: &ServiceConfig, live_listen: SocketAddr) {
    if live_listen == config.options.listen {
        return;
    }
    let Some(cache) = config.log_path.parent() else {
        return;
    };
    post(
        cache,
        &config.options.listen,
        Event::LiveReady {
            listen: live_listen.to_string(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            waiting: config.options.listen.to_string(),
        },
    );
}

pub(super) fn swap_complete(config: &ServiceConfig) {
    let Some(cache) = config.log_path.parent() else {
        return;
    };
    post(
        cache,
        &config.options.listen,
        Event::SwapComplete {
            listen: config.options.listen.to_string(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
        },
    );
}

#[cfg(test)]
pub(super) fn should_emit(event: &Event, last: Option<&LastNotify>) -> bool {
    should_emit_at(event, last, now_unix())
}

pub(super) fn should_emit_at(event: &Event, last: Option<&LastNotify>, now_unix: u64) -> bool {
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

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub(super) fn post(cache: &Path, listen: &SocketAddr, event: Event) {
    if !super::macos_notify_dispatch::notifications_enabled() {
        return;
    }
    super::macos_notify_dispatch::post(cache, listen, event);
}

pub(super) fn post_in_process(cache: &Path, listen: &SocketAddr, event: Event) {
    if !super::macos_notify_dispatch::notifications_enabled() {
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

pub(super) fn deliver(notification: &Notification) -> Result<()> {
    let status = spawn_notification(notification).context("start osascript")?;
    deliver_status(status)
}

pub(super) fn read_last(cache: &Path, listen: &SocketAddr) -> Option<LastNotify> {
    let bytes = fs::read(launcher_logs::hot_swap_notify_path(cache, listen)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_last(cache: &Path, listen: &SocketAddr, last: &LastNotify) -> Result<()> {
    fs::create_dir_all(cache).context("create macOS notification cache")?;
    fs::write(
        launcher_logs::hot_swap_notify_path(cache, listen),
        serde_json::to_vec(last).context("encode macOS notification dedup state")?,
    )
    .context("write macOS notification dedup state")
}

fn spawn_notification(notification: &Notification) -> std::io::Result<ExitStatus> {
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

fn record_event(event: &Event) {
    #[cfg(test)]
    EVENTS.with(|events| events.borrow_mut().push(event.clone()));
    let _ = event;
}

#[cfg(test)]
type TestSpawnFn = fn(&Notification) -> std::io::Result<ExitStatus>;

#[cfg(test)]
thread_local! {
    static EVENTS: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_SPAWN: std::cell::Cell<Option<TestSpawnFn>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn take_events() -> Vec<Event> {
    EVENTS.with(|events| events.borrow_mut().drain(..).collect())
}

#[cfg(test)]
fn synthetic_success() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[cfg(test)]
#[path = "macos_notify_test_hooks.rs"]
mod test_hooks;
#[cfg(test)]
pub(crate) use test_hooks::{TestEvents, TestSpawn};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "macos_notify_tests.rs"]
mod tests;
