#![cfg_attr(coverage_nightly, coverage(off))]

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
const FIXTURE_KIND: &str = "CLAUDEX_PROCESS_FIXTURE_KIND";
#[cfg(unix)]
const FIXTURE_IDS: &str = "CLAUDEX_PROCESS_FIXTURE_IDS";

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
struct ProcessIds {
    leader: i32,
    leader_group: i32,
    child: i32,
    child_group: i32,
    child_session: i32,
}

#[cfg(unix)]
impl ProcessIds {
    fn read(path: &Path) -> Self {
        let contents = std::fs::read_to_string(path).expect("read process fixture IDs");
        let mut values = contents
            .split_whitespace()
            .map(|value| value.parse::<i32>().expect("numeric process fixture ID"));
        let ids = Self {
            leader: values.next().expect("fixture leader PID"),
            leader_group: values.next().expect("fixture leader group"),
            child: values.next().expect("fixture child PID"),
            child_group: values.next().expect("fixture child group"),
            child_session: values.next().expect("fixture child session"),
        };
        assert!(values.next().is_none(), "only five process fixture IDs");
        ids
    }
}

#[cfg(unix)]
struct ProcessCleanup {
    pid: i32,
}

#[cfg(unix)]
impl ProcessCleanup {
    const fn new(pid: i32) -> Self {
        Self { pid }
    }

    fn stop(&mut self) {
        if self.pid <= 1 {
            return;
        }
        // SAFETY: this guard stores one PID created by its test fixture.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
        }
        assert!(
            wait_for_process_stop(self.pid),
            "stop fixture PID {}",
            self.pid
        );
        self.pid = 0;
    }

    fn disarm(&mut self) {
        self.pid = 0;
    }
}

#[cfg(unix)]
impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        if self.pid > 1 {
            // SAFETY: best-effort cleanup of the exact PID owned by this test.
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
fn process_is_live(pid: i32) -> bool {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output();
    let Ok(output) = output else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .is_some_and(|state| !state.starts_with('Z'))
}

#[cfg(unix)]
fn wait_for_process_stop(pid: i32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if !process_is_live(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::yield_now();
    }
}

#[cfg(unix)]
fn fixture_command(kind: &str, ids: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args(["--ignored", "process_tree_fixture", "--test-threads=1"])
        .env(FIXTURE_KIND, kind)
        .env(FIXTURE_IDS, ids);
    command
}

#[cfg(unix)]
fn write_fixture_ids(path: PathBuf, child: i32) {
    // SAFETY: these process queries do not mutate memory and target live fixture PIDs.
    let leader = unsafe { libc::getpid() };
    // SAFETY: getpgid/getsid only query the exact live fixture PIDs.
    let leader_group = unsafe { libc::getpgid(leader) };
    let child_group = unsafe { libc::getpgid(child) };
    let child_session = unsafe { libc::getsid(child) };
    std::fs::write(
        path,
        format!("{leader} {leader_group} {child} {child_group} {child_session}\n"),
    )
    .expect("write process fixture IDs");
}

#[cfg(unix)]
#[test]
#[ignore = "helper process launched by process-runner tests"]
fn process_tree_fixture() {
    let kind = std::env::var(FIXTURE_KIND).expect("fixture kind");
    let ids = PathBuf::from(std::env::var_os(FIXTURE_IDS).expect("fixture IDs path"));
    let escaped = kind == "escaped-normal";
    let leader_blocks = kind == "same-session-timeout";
    let mut ready = [-1; 2];
    // SAFETY: `ready` points to two valid integers for the new pipe descriptors.
    assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
    // SAFETY: the child branch uses only async-signal-safe libc operations.
    let child = unsafe { libc::fork() };
    assert!(child >= 0, "fork process fixture child");
    if child == 0 {
        // SAFETY: these are the exact descriptors and process created above.
        unsafe {
            libc::close(ready[0]);
            let boundary = if escaped {
                libc::setsid()
            } else {
                libc::setpgid(0, 0)
            };
            if boundary == -1 {
                libc::_exit(110);
            }
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            let ready_byte = [1_u8];
            if libc::write(ready[1], ready_byte.as_ptr().cast(), 1) != 1 {
                libc::_exit(111);
            }
            libc::close(ready[1]);
            loop {
                libc::pause();
            }
        }
    }

    // SAFETY: the parent owns both pipe descriptors returned above.
    unsafe {
        libc::close(ready[1]);
    }
    let mut ready_byte = [0_u8];
    // SAFETY: the child writes exactly one byte before entering its pause loop.
    let ready_count = unsafe { libc::read(ready[0], ready_byte.as_mut_ptr().cast(), 1) };
    // SAFETY: the read end is no longer needed after the one-byte handshake.
    unsafe {
        libc::close(ready[0]);
    }
    assert_eq!(ready_count, 1, "process fixture child reached its boundary");
    write_fixture_ids(ids, child);
    if leader_blocks {
        // SAFETY: the fixture leader deliberately resists TERM for escalation testing.
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            loop {
                libc::pause();
            }
        }
    }
}

#[test]
fn returns_exit_code_and_both_output_streams() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf stdout; printf stderr >&2; exit 7"]);
    let (code, stdout, stderr) = run_with_timeout(command, Duration::from_secs(2)).unwrap();
    assert_eq!(code, 7);
    assert_eq!(stdout, "stdout");
    assert_eq!(stderr, "stderr");
}

#[test]
fn bounded_stdin_stream_reaches_the_child_without_a_writer_thread() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "read value; printf '%s' \"$value\""]);
    let output = run_before_with_input(
        command,
        Deadline::after(Duration::from_secs(4)),
        Duration::from_secs(2),
        b"embedded-oauth\n",
    )
    .unwrap();
    assert_eq!(output, (0, "embedded-oauth".to_owned(), String::new()));
}

#[cfg(unix)]
#[test]
fn normal_leader_exit_cleans_a_distinct_group_in_the_same_session() {
    let root = tempfile::tempdir().expect("process fixture directory");
    let ids_path = root.path().join("process.ids");
    let command = fixture_command("same-session-normal", &ids_path);
    let (code, _, _) = run_with_timeout(command, Duration::from_secs(3)).unwrap();
    let ids = ProcessIds::read(&ids_path);
    let mut cleanup = ProcessCleanup::new(ids.child);

    assert_eq!(code, 0);
    assert_eq!(ids.leader_group, ids.leader, "setsid leader group");
    assert_eq!(ids.child_group, ids.child, "fixture uses a distinct group");
    assert_eq!(
        ids.child_session, ids.leader,
        "fixture remains in runner session"
    );
    assert!(!process_is_live(ids.child));
    cleanup.disarm();
}

#[cfg(unix)]
#[test]
fn timeout_kills_every_process_group_in_the_session() {
    let root = tempfile::tempdir().expect("process fixture directory");
    let ids_path = root.path().join("process.ids");
    let command = fixture_command("same-session-timeout", &ids_path);
    let error = run_with_timeout(command, Duration::from_secs(1)).unwrap_err();
    let ids = ProcessIds::read(&ids_path);
    let mut child_cleanup = ProcessCleanup::new(ids.child);
    let mut leader_cleanup = ProcessCleanup::new(ids.leader);

    assert!(error.to_string().contains("subprocess timed out"));
    assert_eq!(ids.leader_group, ids.leader);
    assert_eq!(ids.child_group, ids.child);
    assert_eq!(ids.child_session, ids.leader);
    assert!(!process_is_live(ids.leader));
    assert!(!process_is_live(ids.child));
    leader_cleanup.disarm();
    child_cleanup.disarm();
}

#[cfg(unix)]
#[test]
fn term_grace_allows_a_cooperative_process_to_record_shutdown() {
    let root = tempfile::tempdir().expect("TERM fixture directory");
    let marker = root.path().join("term.marker");
    let script = "trap 'printf term > \"$1\"; exit 0' TERM; while :; do :; done";
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script, "fixture", marker.to_str().unwrap()]);

    let error = run_with_timeout(command, Duration::from_millis(500)).unwrap_err();
    assert!(error.to_string().contains("subprocess timed out"));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "term");
}

#[cfg(unix)]
#[test]
fn escaped_session_pipe_holder_cannot_extend_the_output_deadline() {
    let root = tempfile::tempdir().expect("escaped fixture directory");
    let ids_path = root.path().join("process.ids");
    let command = fixture_command("escaped-normal", &ids_path);
    let started = Instant::now();
    let (code, _, _) = run_with_timeout(command, Duration::from_secs(3)).unwrap();
    let elapsed = started.elapsed();
    let ids = ProcessIds::read(&ids_path);
    let mut cleanup = ProcessCleanup::new(ids.child);

    assert_eq!(code, 0);
    assert_eq!(ids.leader_group, ids.leader);
    assert_eq!(ids.child_group, ids.child);
    assert_eq!(ids.child_session, ids.child, "child escaped via setsid");
    assert!(
        process_is_live(ids.child),
        "escaped holder remains out of scope"
    );
    assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
    cleanup.stop();
}
