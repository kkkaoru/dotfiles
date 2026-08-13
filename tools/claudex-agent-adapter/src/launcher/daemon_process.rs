use std::{ffi::OsStr, path::Path, process, process::Command};

#[cfg(unix)]
use std::time::Duration;

#[path = "daemon_process_signals.rs"]
mod signals;
#[cfg(unix)]
use signals::process_is_alive;
pub(super) fn started_daemon_cleanup(pid: u32) -> impl FnOnce(u32) {
    let identity = signals::capture_started_daemon_identity(pid);
    move |_| {
        if let Some(identity) = identity {
            signals::terminate_started_daemon_identity(&identity);
        }
    }
}
#[cfg(all(test, unix))]
use signals::{
    is_process_still_alive, is_process_zombie, kill_process, kill_process_group,
    process_group_is_alive,
};
use signals::{request_graceful_shutdown_with_signal, terminate_with_escalation};

const LEGACY_ADAPTER_NAMES: &[&str] = &["claudex-app-server-adapter"];
#[cfg(unix)]
pub(super) const STALE_TERMINATE_TIMEOUT: Duration = Duration::from_millis(250);
#[cfg(unix)]
pub(super) const STALE_TERMINATE_POLL: Duration = Duration::from_millis(10);

pub(super) fn matches(pid: u32, executable: &Path) -> bool {
    fields_match(
        process_field(pid, "comm="),
        process_field(pid, "command="),
        executable,
    )
}

/// Whether `pid` still exists (and is not a zombie). Used to fail warm-start
/// waits early when the child exits before `/health` can answer.
pub(super) fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        process_is_alive(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn fields_match(program: Option<String>, command: Option<String>, executable: &Path) -> bool {
    let Some(program) = program else {
        return false;
    };
    let Some(command) = command else {
        return false;
    };
    command_matches(&program, &command, executable)
}

pub(super) fn terminate(pid: u32) {
    if !is_signalable_pid(pid) || pid == process::id() || is_launch_process(pid) {
        return;
    }
    terminate_with_escalation(pid);
}

/// Terminate a process returned by the detached daemon spawner.
///
/// The detached-session check prevents a recycled or unrelated PID from being
/// signalled, while still allowing cleanup when the session leader has exited
/// but provider process groups remain in its session.
#[cfg(test)]
pub(super) fn terminate_started_daemon(pid: u32) {
    if !is_signalable_pid(pid) || pid == process::id() {
        return;
    }
    if let Some(identity) = signals::capture_started_daemon_identity(pid) {
        signals::terminate_started_daemon_identity(&identity);
    }
}

/// Terminate a retained `serve` daemon by pid without needing the executable
/// path. Refuses launch TUIs and any non-adapter process (test fixtures use
/// pid 1, etc.).
pub(crate) fn terminate_retained_serve(pid: u32) {
    if !is_signalable_pid(pid) || pid == process::id() || !is_serve_process(pid) {
        return;
    }
    terminate_with_escalation(pid);
}

fn is_serve_process(pid: u32) -> bool {
    process_field(pid, "command=").is_some_and(|command| is_serve_command_line(&command))
}

fn is_serve_command_line(command: &str) -> bool {
    let mut fields = command.split_whitespace();
    let Some(executable) = fields.next() else {
        return false;
    };
    executable.rsplit('/').next() == Some("claudex-agent-adapter") && fields.next() == Some("serve")
}

/// Ask an adapter to stop accepting new requests while its HTTP server drains
/// the ones it has already accepted. Unlike [`terminate`], a handover must not
/// signal the adapter's process group or escalate to SIGKILL: either action can
/// cut off an in-flight response before Axum's graceful shutdown completes.
pub(super) fn request_graceful_shutdown(pid: u32) {
    if !is_signalable_pid(pid) || pid == process::id() || is_launch_process(pid) {
        return;
    }
    request_graceful_shutdown_with_signal(pid);
}

fn is_launch_process(pid: u32) -> bool {
    process_field(pid, "command=").is_some_and(|command| is_launch_command_line(&command))
}

fn is_launch_command_line(command: &str) -> bool {
    let mut fields = command.split_whitespace();
    let Some(executable) = fields.next() else {
        return false;
    };
    executable.rsplit('/').next() == Some("claudex-agent-adapter")
        && fields.next() == Some("launch")
}

fn is_signalable_pid(pid: u32) -> bool {
    pid != 0 && pid <= i32::MAX as u32
}

fn process_field(pid: u32, field: &str) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn command_matches(program: &str, command: &str, executable: &Path) -> bool {
    // Prefer argv0 from `command=` — macOS `comm=` truncates, and ensure may
    // spawn from ~/.cache/claudex/bin while the live daemon is still under
    // ~/.cargo/bin.
    let mut fields = command.split_whitespace();
    let Some(argv0) = fields.next().map(Path::new) else {
        return false;
    };
    let subcommand = fields.next();
    let program = Path::new(program);
    let current = argv0 == executable || program == executable;
    let expected = executable.file_name();
    let same_binary_name = expected
        .is_some_and(|name| argv0.file_name() == Some(name) || program.file_name() == Some(name));
    let renamed = (argv0.parent() == executable.parent()
        || program.parent() == executable.parent())
        && LEGACY_ADAPTER_NAMES.iter().any(|legacy| {
            let legacy = OsStr::new(legacy);
            argv0.file_name() == Some(legacy) || program.file_name() == Some(legacy)
        });
    (current || renamed || same_binary_name) && subcommand == Some("serve")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "daemon_process_signal_edges_tests.rs"]
mod signal_edges_tests;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "daemon_process_tests.rs"]
mod tests;
