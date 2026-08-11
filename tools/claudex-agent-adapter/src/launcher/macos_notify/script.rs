use std::{
    path::PathBuf,
    process::{Command, ExitStatus},
};

use anyhow::{Result, bail};

use super::{COMPLETE_SUBTITLE, Event, LIVE_SUBTITLE, Notification, TITLE, WAITING_SUBTITLE};

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
            body: format!("build {build_id} を即時利用 · live {listen} · waiting {waiting}"),
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
