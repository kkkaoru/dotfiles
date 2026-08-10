use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, launcher_lock, launcher_logs};

const TITLE: &str = "claudex";
const WAITING_SUBTITLE: &str = "ビルド完了・待機中";
const LIVE_SUBTITLE: &str = "live 更新完了";
const COMPLETE_SUBTITLE: &str = "差し替え完了";
/// Rapid `cargo install` / hot-swap bursts mint a new build_id each time.
/// Keep macOS alerts quiet until this gap of silence across all listen ports.
const RAPID_REBUILD_COOLDOWN_SECS: u64 = 5 * 60;

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
    if last.emitted_unix > 0
        && now_unix.saturating_sub(last.emitted_unix) < RAPID_REBUILD_COOLDOWN_SECS
    {
        return false;
    }
    true
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub(super) fn notification(event: &Event) -> Notification {
    match event {
        Event::WaitingForIdle {
            listen,
            build_id,
            waiter_pid,
        } => Notification {
            title: format!("{TITLE} · {listen}"),
            subtitle: format!("{WAITING_SUBTITLE} · build {build_id}"),
            body: format!(
                "build {build_id} へ差し替え待機中 · listen {listen} · waiter pid {waiter_pid}"
            ),
        },
        Event::LiveReady {
            listen,
            build_id,
            waiting,
        } => Notification {
            title: format!("{TITLE} · {listen}"),
            subtitle: format!("{LIVE_SUBTITLE} · build {build_id}"),
            body: format!(
                "build {build_id} を即時利用 · live {listen} · waiting {waiting}"
            ),
        },
        Event::SwapComplete { listen, build_id } => Notification {
            title: format!("{TITLE} · {listen}"),
            subtitle: format!("{COMPLETE_SUBTITLE} · build {build_id}"),
            body: format!("build {build_id} へ差し替えました · listen {listen}"),
        },
    }
}

pub(super) fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn osascript_arguments(notification: &Notification) -> Vec<String> {
    vec![
        "-e".to_owned(),
        format!(
            "display notification \"{}\" with title \"{}\" subtitle \"{}\"",
            escape_applescript(&notification.body),
            escape_applescript(&notification.title),
            escape_applescript(&notification.subtitle),
        ),
    ]
}

pub(super) fn osascript_program() -> PathBuf {
    PathBuf::from("osascript")
}

pub(super) fn osascript_command(notification: &Notification) -> Command {
    let mut command = Command::new(osascript_program());
    command.args(osascript_arguments(notification));
    command
}

pub(super) fn deliver_status(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("osascript exited {status}")
    }
}

pub(super) fn post(cache: &Path, listen: &SocketAddr, event: Event) {
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
        // Slide the cooldown while installs keep racing so a burst stays quiet
        // until there is a real gap without swap attempts.
        if let Some(mut last) = previous {
            last.emitted_unix = now;
            if let Err(error) = write_last(cache, listen, &last) {
                eprintln!("claudex: macOS notification dedup state failed ({error:#})");
            }
        }
        return;
    }
    record_event(&event);
    let mut last = LastNotify::from(&event);
    last.emitted_unix = now;
    if let Err(error) = write_last(cache, listen, &last) {
        eprintln!("claudex: macOS notification dedup state failed ({error:#})");
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
        return Ok(synthetic_success());
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
thread_local! {
    static EVENTS: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_SPAWN: std::cell::Cell<Option<fn(&Notification) -> std::io::Result<ExitStatus>>> =
        const { std::cell::Cell::new(None) };
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
pub(super) struct TestEvents;

#[cfg(test)]
impl TestEvents {
    pub(super) fn capture() -> Self {
        take_events();
        Self
    }

    pub(super) fn take(&self) -> Vec<Event> {
        take_events()
    }
}

#[cfg(test)]
impl Drop for TestEvents {
    fn drop(&mut self) {
        take_events();
    }
}

#[cfg(test)]
pub(super) struct TestSpawn;

#[cfg(test)]
impl TestSpawn {
    pub(super) fn arm(spawn: fn(&Notification) -> std::io::Result<ExitStatus>) -> Self {
        take_events();
        TEST_SPAWN.with(|cell| cell.set(Some(spawn)));
        Self
    }

    pub(super) fn take_events(&self) -> Vec<Event> {
        take_events()
    }
}

#[cfg(test)]
impl Drop for TestSpawn {
    fn drop(&mut self) {
        TEST_SPAWN.with(|cell| cell.set(None));
        take_events();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "macos_notify_tests.rs"]
mod tests;
