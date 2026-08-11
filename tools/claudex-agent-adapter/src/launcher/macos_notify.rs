use std::net::SocketAddr;
#[cfg(test)]
use std::process::ExitStatus;
use serde::{Deserialize, Serialize};

use super::ServiceConfig;
#[cfg(test)]
use super::launcher_logs;

mod script;
mod delivery;
pub(super) use delivery::{post, post_in_process};
#[cfg(test)]
use delivery::{deliver, read_last};
#[cfg(test)]
use script::{deliver_status, escape_applescript, notification, osascript_command, osascript_program};

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


mod emit;
pub(super) use emit::{now_unix, should_emit_at};
#[cfg(test)]
pub(super) use emit::should_emit;

#[cfg(test)]
type TestSpawnFn = fn(&Notification) -> std::io::Result<ExitStatus>;

#[cfg(test)]
thread_local! {
    pub(super) static EVENTS: std::cell::RefCell<Vec<Event>> = const { std::cell::RefCell::new(Vec::new()) };
    pub(super) static TEST_SPAWN: std::cell::Cell<Option<TestSpawnFn>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn take_events() -> Vec<Event> {
    EVENTS.with(|events| events.borrow_mut().drain(..).collect())
}

#[cfg(test)]
pub(super) fn synthetic_success() -> ExitStatus {
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
