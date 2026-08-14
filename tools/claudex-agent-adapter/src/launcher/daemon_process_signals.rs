use std::process::Command;

#[cfg(unix)]
use super::{STALE_TERMINATE_POLL, STALE_TERMINATE_TIMEOUT};
#[cfg(unix)]
use std::{collections::BTreeSet, thread, time::Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StartedDaemonIdentity {
    pid: u32,
    start_token: String,
}

pub(super) fn capture_started_daemon_identity(pid: u32) -> Option<StartedDaemonIdentity> {
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    #[cfg(unix)]
    return owned_daemon_session(pid)
        .and_then(|_| process_start_token(pid))
        .map(|start_token| StartedDaemonIdentity { pid, start_token });
    #[cfg(not(unix))]
    process_start_token(pid).map(|start_token| StartedDaemonIdentity { pid, start_token })
}

pub(super) fn terminate_started_daemon_identity(identity: &StartedDaemonIdentity) {
    #[cfg(unix)]
    {
        match identity_action(identity, process_start_token(identity.pid).as_deref()) {
            IdentityAction::TerminateLeader => {
                terminate_with_escalation(identity.pid);
            }
            IdentityAction::RefuseReusedPid => {
                // The PID now names a different process. Never signal it or a
                // session inferred only from the recycled numeric identifier.
            }
            IdentityAction::TerminateOrphanedSession => {
                // The captured leader exited. Clean only groups whose current
                // members are still proven to belong to its old session; do
                // not signal the numeric leader PID or `-pid` directly.
                terminate_orphaned_session(identity.pid as i32);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = identity;
    }
}

fn identity_action(identity: &StartedDaemonIdentity, current: Option<&str>) -> IdentityAction {
    match current {
        Some(current) if current == identity.start_token => IdentityAction::TerminateLeader,
        Some(_) => IdentityAction::RefuseReusedPid,
        None => IdentityAction::TerminateOrphanedSession,
    }
}
#[derive(Debug, Eq, PartialEq)]
enum IdentityAction {
    TerminateLeader,
    RefuseReusedPid,
    TerminateOrphanedSession,
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn start_token_fails_closed_when_a_pid_is_reused() {
        let identity = StartedDaemonIdentity {
            pid: std::process::id(),
            start_token: "never-current-start-token".to_owned(),
        };
        assert_eq!(
            identity_action(&identity, Some("replacement")),
            IdentityAction::RefuseReusedPid
        );
        assert_eq!(
            identity_action(&identity, Some("never-current-start-token")),
            IdentityAction::TerminateLeader
        );
        assert_eq!(
            identity_action(&identity, None),
            IdentityAction::TerminateOrphanedSession
        );
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        terminate_started_daemon_identity(&identity);
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        assert!(process_is_alive(identity.pid));
    }

    #[test]
    fn capture_daemon_identity_rejects_invalid_pids() {
        assert_eq!(capture_started_daemon_identity(0), None);
        assert_eq!(capture_started_daemon_identity(i32::MAX as u32 + 1), None);
    }

    #[cfg(unix)]
    #[test]
    fn process_listing_skips_malformed_rows_and_zombies() {
        let listing = parse_process_listing(b"101 101 S\ninvalid\n102 101 Z\n103 0 R\n104 104 R\n");
        assert_eq!(listing.len(), 4);
        assert!(listing[1].zombie);
        assert_eq!(listing[2].process_group, 0);
        assert!(live_process_groups_in_session(-1).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn process_checks_fail_closed_for_missing_processes() {
        let missing_pid = 99_999_999;
        assert!(!process_is_alive(missing_pid));
        assert!(!is_process_zombie(missing_pid));
        assert!(!process_group_is_alive(i32::MAX));
        assert_eq!(session_id(-1), None);
        assert_eq!(owned_daemon_session(u32::MAX), None);
    }
}

#[cfg(target_vendor = "apple")]
fn process_start_token(pid: u32) -> Option<String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(format!(
        "{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

#[cfg(target_os = "linux")]
fn process_start_token(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.get(stat.rfind(')')? + 2..)?;
    // `/proc/<pid>/stat` field 22 is starttime; `after_name` starts at field 3.
    after_name.split_whitespace().nth(19).map(ToOwned::to_owned)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn process_start_token(_pid: u32) -> Option<String> {
    None
}

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
    let owned_session = owned_daemon_session(pid);
    signal_termination_scope(libc::SIGTERM, pid, process_group, owned_session);
    let deadline = Instant::now() + STALE_TERMINATE_TIMEOUT;
    while termination_scope_is_alive(pid, process_group, owned_session) {
        if Instant::now() >= deadline {
            break;
        }
        // Provider children can move into their own process groups after the
        // first snapshot. Only re-signal groups proven to remain in this
        // daemon's detached session.
        signal_owned_session_groups(libc::SIGTERM, owned_session);
        thread::sleep(STALE_TERMINATE_POLL);
    }
    if !termination_scope_is_alive(pid, process_group, owned_session) {
        return;
    }

    signal_termination_scope(libc::SIGKILL, pid, process_group, owned_session);
    let deadline = Instant::now() + STALE_TERMINATE_TIMEOUT;
    while termination_scope_is_alive(pid, process_group, owned_session) && Instant::now() < deadline
    {
        signal_owned_session_groups(libc::SIGKILL, owned_session);
        thread::sleep(STALE_TERMINATE_POLL);
    }
}

#[cfg(unix)]
fn signal_termination_scope(signal: i32, pid: u32, process_group: i32, session: Option<i32>) {
    kill_process(signal, pid);
    kill_process_group(signal, process_group);
    signal_owned_session_groups(signal, session);
}

#[cfg(unix)]
fn signal_owned_session_groups(signal: i32, session: Option<i32>) {
    let Some(session) = session else {
        return;
    };
    for process_group in live_process_groups_in_session(session) {
        kill_process_group(signal, process_group);
    }
}

#[cfg(unix)]
fn termination_scope_is_alive(pid: u32, process_group: i32, session: Option<i32>) -> bool {
    process_is_alive(pid)
        || process_group_is_alive(process_group)
        || session.is_some_and(session_has_live_members)
}

#[cfg(unix)]
fn owned_daemon_session(pid: u32) -> Option<i32> {
    let pid = i32::try_from(pid).ok()?;
    (session_id(pid) == Some(pid) || session_has_live_members(pid)).then_some(pid)
}

#[cfg(unix)]
fn session_has_live_members(session: i32) -> bool {
    !live_process_groups_in_session(session).is_empty()
}

#[cfg(unix)]
fn terminate_orphaned_session(session: i32) {
    let groups = live_process_groups_in_session(session);
    if groups.is_empty() {
        return;
    }
    for group in &groups {
        kill_process_group(libc::SIGTERM, *group);
    }
    let deadline = Instant::now() + STALE_TERMINATE_TIMEOUT;
    while session_has_live_members(session) && Instant::now() < deadline {
        thread::sleep(STALE_TERMINATE_POLL);
    }
    for group in live_process_groups_in_session(session) {
        kill_process_group(libc::SIGKILL, group);
    }
}

#[cfg(unix)]
fn live_process_groups_in_session(session: i32) -> BTreeSet<i32> {
    process_listing()
        .into_iter()
        .flatten()
        .filter(|process| !process.zombie && session_id(process.pid) == Some(session))
        .map(|process| process.process_group)
        .filter(|&process_group| process_group > 1)
        .collect()
}

#[cfg(unix)]
fn session_id(pid: i32) -> Option<i32> {
    let session = unsafe { libc::getsid(pid) };
    (session >= 0).then_some(session)
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct ProcessListing {
    pid: i32,
    process_group: i32,
    zombie: bool,
}

#[cfg(unix)]
fn process_listing() -> Option<Vec<ProcessListing>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,pgid=,stat="])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_process_listing(&output.stdout))
}

#[cfg(unix)]
fn parse_process_listing(output: &[u8]) -> Vec<ProcessListing> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(ProcessListing {
                pid: fields.next()?.parse().ok()?,
                process_group: fields.next()?.parse().ok()?,
                zombie: fields.next()?.starts_with('Z'),
            })
        })
        .collect()
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
}
