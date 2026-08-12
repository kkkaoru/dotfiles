//! Unix session creation and TERM-to-KILL cleanup.

use super::{Deadline, capture::capture_stdout_bounded};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const TERMINATION_GRACE: Duration = Duration::from_millis(250);
const FORCE_KILL_GRACE: Duration = Duration::from_millis(500);
const SESSION_POLL: Duration = Duration::from_millis(20);
const PROCESS_TABLE_TIMEOUT: Duration = Duration::from_millis(200);

const PROCESS_TABLE_FORMAT: &str = "pid=,pgid=,stat=";

#[derive(Debug, Default, Eq, PartialEq)]
struct SessionSnapshot {
    pids: BTreeSet<i32>,
    process_groups: BTreeSet<i32>,
}

pub(super) fn configure_new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn parse_process_table(table: &str, session_id: i32) -> SessionSnapshot {
    let mut snapshot = SessionSnapshot::default();
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(process_group), Some(state)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(process_group)) = (pid.parse::<i32>(), process_group.parse::<i32>())
        else {
            continue;
        };
        if state.starts_with('Z') || pid <= 1 || process_group <= 1 {
            continue;
        }
        // macOS `ps` exposes `sess` as a kernel session pointer which is 0 for
        // these detached children, not the POSIX SID returned by getsid(2).
        // Query the kernel for each candidate and repeat this check immediately
        // before signaling so a recycled PID is never targeted.
        if unsafe { libc::getsid(pid) } != session_id {
            continue;
        }
        snapshot.pids.insert(pid);
        snapshot.process_groups.insert(process_group);
    }
    snapshot
}

fn live_session(session_id: i32, deadline: Instant) -> Result<SessionSnapshot> {
    let now = Instant::now();
    if now >= deadline {
        bail!("process-table inspection deadline expired");
    }
    let probe_deadline = (now + PROCESS_TABLE_TIMEOUT).min(deadline);
    let mut command = Command::new("/bin/ps");
    command
        .args(["-axo", PROCESS_TABLE_FORMAT])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .current_dir("/");
    let captured =
        capture_stdout_bounded(command, probe_deadline).context("inspect process table")?;
    if !captured.status.success() {
        bail!("process-table inspection exited with {}", captured.status);
    }
    Ok(parse_process_table(
        &String::from_utf8_lossy(&captured.stdout),
        session_id,
    ))
}

fn signal_pid_if_same_session(pid: i32, session_id: i32, signal: i32) -> Result<()> {
    if pid <= 1 || session_id <= 1 {
        bail!("refusing to signal unsafe subprocess PID");
    }
    let actual_session = unsafe { libc::getsid(pid) };
    if actual_session == -1 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error).context("revalidate subprocess session")
        };
    }
    if actual_session != session_id {
        return Ok(());
    }
    if unsafe { libc::kill(pid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error).with_context(|| format!("signal {signal} failed for subprocess PID {pid}"))
    }
}

fn signal_snapshot(snapshot: &SessionSnapshot, session_id: i32, signal: i32) -> Result<()> {
    let mut first_error = None;
    for pid in &snapshot.pids {
        if let Err(error) = signal_pid_if_same_session(*pid, session_id, signal)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn phase_deadline(hard: Instant, grace: Duration) -> Instant {
    (Instant::now() + grace).min(hard)
}

fn signal_until_empty(session_id: i32, signal: i32, phase_end: Instant) -> Result<bool> {
    loop {
        let now = Instant::now();
        if phase_end.saturating_duration_since(now) <= PROCESS_TABLE_TIMEOUT {
            return Ok(false);
        }
        let snapshot = live_session(session_id, phase_end)?;
        if snapshot.pids.is_empty() {
            return Ok(true);
        }
        signal_snapshot(&snapshot, session_id, signal)?;
        let now = Instant::now();
        if now >= phase_end {
            return Ok(false);
        }
        thread::sleep(phase_end.saturating_duration_since(now).min(SESSION_POLL));
    }
}

pub(super) fn terminate_session(session_id: u32, deadline: Deadline) -> Result<()> {
    let session_id = i32::try_from(session_id).context("subprocess session ID exceeds i32")?;
    let hard = deadline.instant();
    match signal_until_empty(
        session_id,
        libc::SIGTERM,
        phase_deadline(hard, TERMINATION_GRACE),
    ) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(error) => {
            let _ = signal_pid_if_same_session(session_id, session_id, libc::SIGKILL);
            return Err(error).context("enumerate subprocess session during TERM cleanup");
        }
    }
    match signal_until_empty(
        session_id,
        libc::SIGKILL,
        phase_deadline(hard, FORCE_KILL_GRACE),
    ) {
        Ok(true) => Ok(()),
        Ok(false) => {
            let snapshot = live_session(session_id, hard)?;
            if snapshot.pids.is_empty() {
                Ok(())
            } else {
                bail!(
                    "subprocess session {session_id} remained after SIGKILL; live_pids={:?}; \
                     live_process_groups={:?}",
                    snapshot.pids,
                    snapshot.process_groups
                )
            }
        }
        Err(error) => Err(error).context("inspect subprocess session after SIGKILL"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_distinct_live_groups_and_excludes_other_sessions_and_zombies() {
        let own_pid = unsafe { libc::getpid() };
        let own_group = unsafe { libc::getpgid(own_pid) };
        let own_session = unsafe { libc::getsid(own_pid) };
        let table =
            format!("{own_pid} {own_group} Ss\n{own_pid} {own_group} Z+\n1 1 S\nmalformed\n");
        let snapshot = parse_process_table(&table, own_session);
        assert_eq!(snapshot.pids, BTreeSet::from([own_pid]));
        assert_eq!(snapshot.process_groups, BTreeSet::from([own_group]));
    }
}
