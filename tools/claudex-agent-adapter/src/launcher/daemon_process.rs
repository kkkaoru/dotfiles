use std::{ffi::OsStr, path::Path, process, process::Command};

#[cfg(unix)]
use std::{
    thread,
    time::{Duration, Instant},
};

const LEGACY_ADAPTER_NAMES: &[&str] = &["claudex-app-server-adapter"];
#[cfg(unix)]
const STALE_TERMINATE_TIMEOUT: Duration = Duration::from_millis(250);
#[cfg(unix)]
const STALE_TERMINATE_POLL: Duration = Duration::from_millis(10);

pub(super) fn matches(pid: u32, executable: &Path) -> bool {
    fields_match(
        process_field(pid, "comm="),
        process_field(pid, "command="),
        executable,
    )
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

#[cfg(unix)]
fn request_graceful_shutdown_with_signal(pid: u32) {
    kill_process(libc::SIGTERM, pid);
}

#[cfg(not(unix))]
fn request_graceful_shutdown_with_signal(pid: u32) {
    use std::process::Stdio;

    let _status = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(unix)]
fn terminate_with_escalation(pid: u32) {
    let process_group = pid as i32;
    kill_process(libc::SIGTERM, pid);
    kill_process_group(libc::SIGTERM, process_group);
    let deadline = Instant::now() + STALE_TERMINATE_TIMEOUT;
    while process_is_alive(pid) || process_group_is_alive(process_group) {
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(STALE_TERMINATE_POLL);
    }
    if process_is_alive(pid) || process_group_is_alive(process_group) {
        kill_process(libc::SIGKILL, pid);
        kill_process_group(libc::SIGKILL, process_group);
    }
}

#[cfg(not(unix))]
fn terminate_with_escalation(pid: u32) {
    use std::process::Stdio;

    let _status = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(unix)]
fn kill_process_group(signal: i32, process_group: i32) {
    let _ = unsafe { libc::kill(-process_group, signal) };
}

#[cfg(unix)]
fn kill_process(signal: i32, target: u32) {
    let _ = unsafe { libc::kill(target as i32, signal) };
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    is_process_still_alive(pid) && !is_process_zombie(pid)
}

#[cfg(unix)]
fn is_process_zombie(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(_) => return false,
    };

    let state = output
        .status
        .success()
        .then(|| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .map(|item| item.to_owned())
        })
        .flatten();
    state.is_some_and(|state| state.starts_with('Z'))
}

fn is_process_still_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }
    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EPERM)
    )
}

#[cfg(unix)]
fn process_group_is_alive(process_group_id: i32) -> bool {
    let output = Command::new("ps")
        .args(["-axo", "pid=,pgid=,stat="])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(_) => {
            return unsafe { libc::kill(-process_group_id, 0) == 0 }
                || matches!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM)
                );
        }
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _pid = fields.next()?;
            let group = fields.next()?;
            let state = fields.next()?;
            Some((group, state))
        })
        .any(|(group, state)| {
            let Ok(group) = group.parse::<i32>() else {
                return false;
            };
            group == process_group_id && state.chars().next().is_none_or(|state| state != 'Z')
        })
        || {
            let result = unsafe { libc::kill(-process_group_id, 0) };
            result == 0
                || matches!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EPERM)
                )
        }
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
    let program = Path::new(program);
    let current = program == executable;
    let same_binary_name = program.file_name() == executable.file_name();
    let renamed = program.parent() == executable.parent()
        && program.file_name().is_some_and(|name| {
            LEGACY_ADAPTER_NAMES
                .iter()
                .any(|legacy| name == OsStr::new(legacy))
        });
    let subcommand = command
        .strip_prefix(&program.to_string_lossy().to_string())
        .and_then(|arguments| arguments.split_ascii_whitespace().next());
    (current || renamed || same_binary_name) && subcommand == Some("serve")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "daemon_process_tests.rs"]
mod tests;
