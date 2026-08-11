use super::*;

#[cfg(unix)]
use std::{
    os::unix::process::CommandExt as _,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[test]
fn matches_live_daemon_when_spawn_path_differs() {
    // Regression: ensure spawn under ~/.cache/claudex/bin must still recognize
    // the live ~/.cargo/bin serve process so handover can release :8318.
    let spawn = Path::new("/Users/test/.cache/claudex/bin/claudex-agent-adapter");
    assert!(command_matches(
        "/Users/test/.cargo/bin/claudex-agent-adapter",
        "/Users/test/.cargo/bin/claudex-agent-adapter serve --listen 127.0.0.1:8318",
        spawn
    ));
    assert!(command_matches(
        "claudex-agent-ad",
        "/Users/test/.cargo/bin/claudex-agent-adapter serve --model auto",
        spawn
    ));
}

#[test]
fn recognizes_current_and_renamed_adapter_daemons() {
    let executable = Path::new("/tmp/claudex-agent-adapter");
    assert!(command_matches(
        "/tmp/claudex-agent-adapter",
        "/tmp/claudex-agent-adapter serve --model current",
        executable
    ));
    assert!(command_matches(
        "/Users/kkk4oru/.cargo/bin/claudex-agent-adapter",
        "/Users/kkk4oru/.cargo/bin/claudex-agent-adapter serve --model legacy",
        executable
    ));
    assert!(command_matches(
        "/tmp/claudex-app-server-adapter",
        "/tmp/claudex-app-server-adapter serve --model legacy",
        executable
    ));
    assert!(!command_matches(
        "/tmp/claudex-agent-adapter",
        "/tmp/claudex-agent-adapter launch --model current",
        executable
    ));
    assert!(is_launch_command_line(
        "/Users/test/.local/bin/claudex-agent-adapter launch --model opus"
    ));
    assert!(!is_launch_command_line(
        "/Users/test/.local/bin/claudex-agent-adapter serve --model opus"
    ));
    assert!(!is_launch_command_line(""));
    assert!(!is_launch_command_line("   "));
    assert!(!command_matches(
        "/usr/local/bin/claudex-app-server-adapter",
        "/usr/local/bin/claudex-app-server-adapter serve --model legacy",
        executable
    ));
    assert!(!command_matches(
        "/tmp/unrelated-adapter",
        "/tmp/unrelated-adapter serve --model current",
        executable
    ));
    assert!(!command_matches("", "", executable));
    assert!(!matches(u32::MAX, executable));
    assert!(!fields_match(
        Some("/tmp/claudex-agent-adapter".to_owned()),
        None,
        executable
    ));
    assert!(!fields_match(
        None,
        Some("/tmp/claudex-agent-adapter serve".to_owned()),
        executable
    ));
    assert!(!fields_match(None, None, executable));
}

#[test]
fn rejects_pids_that_cannot_be_safely_signalled() {
    assert!(!is_signalable_pid(0));
    assert!(!is_signalable_pid(u32::MAX));
    assert!(is_signalable_pid(1));
    assert!(is_signalable_pid(i32::MAX as u32));
    terminate(0);
    terminate(u32::MAX);
    terminate(std::process::id());
}

#[test]
fn rejects_the_current_test_process_as_a_daemon() {
    let executable = std::env::current_exe().expect("locate test executable");
    assert!(!matches(std::process::id(), &executable));
}

#[cfg(unix)]
#[test]
fn reports_an_absent_process_group_as_not_alive() {
    assert!(!process_group_is_alive(i32::MAX));
    assert!(!is_process_still_alive(i32::MAX as u32));
    assert!(!is_process_zombie(i32::MAX as u32));
}

#[cfg(unix)]
#[test]
#[allow(clippy::zombie_processes)]
fn skips_terminate_and_graceful_shutdown_for_launch_process() {
    let root = tempfile::tempdir().expect("launch process fixture");
    let program = root.path().join("claudex-agent-adapter");
    let mut compile = Command::new("cc")
        .args(["-x", "c", "-o"])
        .arg(&program)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start cc for launch fixture");
    {
        use std::io::Write as _;
        let stdin = compile.stdin.as_mut().expect("cc stdin");
        stdin
            .write_all(b"#include <unistd.h>\nint main(void){for(;;) sleep(1); return 0;}\n")
            .expect("write launch fixture source");
    }
    let status = compile.wait_with_output().expect("wait for cc");
    assert!(
        status.status.success(),
        "cc failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    let mut child = Command::new(&program)
        .args(["launch", "--model", "main"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start launch process fixture");
    let pid = child.id();
    assert!(is_launch_process(pid));
    terminate(pid);
    request_graceful_shutdown(pid);
    assert!(
        process_is_alive(pid),
        "launch processes must not be signalled by daemon helpers"
    );
    kill_process(libc::SIGKILL, pid);
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
#[allow(clippy::zombie_processes)]
fn graceful_shutdown_leaves_other_process_group_members_running() {
    let root = tempfile::tempdir().expect("graceful shutdown fixture");
    let child_pid_file = root.path().join("child.pid");
    let ready = root.path().join("ready");
    let script =
        "trap 'exit 0' TERM\nsleep 100 &\necho $! > \"$1\"\n: > \"$2\"\nwhile :; do sleep 1; done";
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            script,
            "sh",
            child_pid_file.to_str().expect("child pid path"),
            ready.to_str().expect("ready path"),
        ])
        .process_group(0);
    let mut leader = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start graceful shutdown fixture");
    let process_group = leader.id().try_into().expect("process group fits in i32");
    let _cleanup = TestProcessGroupCleanup(process_group);
    let child_pid = wait_for_file_pid(&child_pid_file, &ready);

    request_graceful_shutdown(leader.id());
    assert!(leader.wait().expect("reap graceful leader").success());
    assert!(process_is_alive(child_pid));

    kill_process_group(libc::SIGKILL, process_group);
    wait_until_process_stops(child_pid);
    assert!(!process_is_alive(child_pid));

    request_graceful_shutdown(0);
    request_graceful_shutdown(u32::MAX);
    request_graceful_shutdown(std::process::id());
}

#[cfg(unix)]
fn wait_for_file_pid(child_pid_file: &Path, ready: &Path) -> u32 {
    for _ in 0..100 {
        match read_ready_pid(child_pid_file, ready) {
            Some(pid) => return pid,
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("graceful shutdown fixture did not become ready")
}

#[cfg(unix)]
fn read_ready_pid(child_pid_file: &Path, ready: &Path) -> Option<u32> {
    ready
        .exists()
        .then(|| std::fs::read_to_string(child_pid_file).ok())
        .flatten()
        .and_then(|pid| pid.trim().parse().ok())
}

#[cfg(unix)]
fn wait_until_process_stops(pid: u32) {
    for _ in 0..100 {
        match process_is_alive(pid) {
            false => return,
            true => thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[cfg(unix)]
struct TestProcessGroupCleanup(i32);

#[cfg(unix)]
impl Drop for TestProcessGroupCleanup {
    fn drop(&mut self) {
        let _result = unsafe { libc::kill(-self.0, libc::SIGKILL) };
    }
}
