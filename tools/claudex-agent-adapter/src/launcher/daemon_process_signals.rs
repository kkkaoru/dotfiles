use std::process::Command;

#[cfg(unix)]
use std::{thread, time::Instant};

#[cfg(unix)]
use super::{STALE_TERMINATE_POLL, STALE_TERMINATE_TIMEOUT};

#[cfg(unix)]
pub(super) fn request_graceful_shutdown_with_signal(pid: u32) {
    kill_process(libc::SIGTERM, pid);
}

#[cfg(not(unix))]
pub(super) fn request_graceful_shutdown_with_signal(pid: u32) {
    use std::process::Stdio;

    let _status = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(unix)]
pub(super) fn terminate_with_escalation(pid: u32) {
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
pub(super) fn terminate_with_escalation(pid: u32) {
    use std::process::Stdio;

    let _status = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(unix)]
pub(super) fn kill_process_group(signal: i32, process_group: i32) {
    let _ = unsafe { libc::kill(-process_group, signal) };
}

#[cfg(unix)]
pub(super) fn kill_process(signal: i32, target: u32) {
    let _ = unsafe { libc::kill(target as i32, signal) };
}

#[cfg(unix)]
pub(super) fn process_is_alive(pid: u32) -> bool {
    is_process_still_alive(pid) && !is_process_zombie(pid)
}

#[cfg(unix)]
pub(super) fn is_process_zombie(pid: u32) -> bool {
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

pub(super) fn is_process_still_alive(pid: u32) -> bool {
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
pub(super) fn process_group_is_alive(process_group_id: i32) -> bool {
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
