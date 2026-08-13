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
fn serve_command_line_detects_retained_daemons_only() {
    assert!(is_serve_command_line(
        "/tmp/claudex-agent-adapter serve --listen 127.0.0.1:59607"
    ));
    assert!(!is_serve_command_line(
        "/tmp/claudex-agent-adapter launch --resume session"
    ));
    assert!(!is_serve_command_line("/sbin/launchd"));
    // Fixture pids (1) and unrelated processes must never be terminated.
    terminate_retained_serve(0);
    terminate_retained_serve(1);
}

#[test]
fn capture_started_daemon_identity_rejects_unusable_pids() {
    assert!(super::signals::capture_started_daemon_identity(0).is_none());
    assert!(super::signals::capture_started_daemon_identity(u32::MAX).is_none());
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
    terminate_started_daemon(0);
    terminate_started_daemon(u32::MAX);
    terminate_started_daemon(std::process::id());
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
#[test]
fn started_daemon_cleanup_ignores_a_group_outside_a_detached_session() {
    let mut child = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ordinary process group");
    let process_group = child.id().try_into().expect("process group fits in i32");
    let _cleanup = TestProcessGroupCleanup(process_group);

    terminate_started_daemon(child.id());
    assert!(
        process_is_alive(child.id()),
        "an ordinary process group must not be treated as a detached daemon session"
    );
    kill_process_group(libc::SIGKILL, process_group);
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn forced_termination_kills_descendant_groups_only_in_the_daemon_session() {
    let root = tempfile::tempdir().expect("daemon session fixture");
    let fixture = compile_daemon_session_fixture(root.path());
    let pid_file = root.path().join("provider.pids");
    let mut command = Command::new(&fixture);
    command.arg(&pid_file);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut daemon = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn detached daemon fixture");
    let daemon_pid = daemon.id();
    let _daemon_cleanup = DaemonSessionCleanup {
        daemon_pid,
        pid_file: pid_file.clone(),
    };
    let (provider_pid, grandchild_pid) = wait_for_pid_pair(&pid_file);

    let mut unrelated = Command::new("sh")
        .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn unrelated process group");
    let unrelated_group = unrelated.id().try_into().expect("unrelated group fits");
    let _unrelated_cleanup = TestProcessGroupCleanup(unrelated_group);

    assert_eq!(
        unsafe { libc::getsid(daemon_pid as i32) },
        daemon_pid as i32
    );
    assert_eq!(
        unsafe { libc::getsid(provider_pid as i32) },
        daemon_pid as i32
    );
    assert_eq!(
        unsafe { libc::getpgid(provider_pid as i32) },
        provider_pid as i32
    );
    assert_eq!(
        unsafe { libc::getpgid(grandchild_pid as i32) },
        provider_pid as i32
    );
    assert_ne!(
        unsafe { libc::getsid(unrelated.id() as i32) },
        daemon_pid as i32
    );

    terminate_started_daemon(daemon_pid);
    wait_until_process_stops(provider_pid);
    wait_until_process_stops(grandchild_pid);
    assert!(!process_is_alive(provider_pid));
    assert!(!process_is_alive(grandchild_pid));
    assert!(
        process_is_alive(unrelated.id()),
        "cleanup must not signal an unrelated session"
    );

    let _ = daemon.wait();
    kill_process_group(libc::SIGKILL, unrelated_group);
    let _ = unrelated.wait();
}

#[cfg(unix)]
fn compile_daemon_session_fixture(root: &Path) -> std::path::PathBuf {
    let executable = root.join("daemon-session-fixture");
    let source = br#"
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static void wait_forever(void) { for (;;) pause(); }

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    signal(SIGTERM, SIG_IGN);
    pid_t provider = fork();
    if (provider < 0) return 3;
    if (provider == 0) {
        if (setpgid(0, 0) != 0) _exit(4);
        signal(SIGTERM, SIG_IGN);
        pid_t grandchild = fork();
        if (grandchild < 0) _exit(5);
        if (grandchild == 0) {
            signal(SIGTERM, SIG_IGN);
            wait_forever();
        }
        FILE *pids = fopen(argv[1], "w");
        if (pids == NULL) _exit(6);
        fprintf(pids, "%d %d\n", getpid(), grandchild);
        fclose(pids);
        wait_forever();
    }
    wait_forever();
}
"#;
    let mut compiler = Command::new("cc")
        .args(["-x", "c", "-o"])
        .arg(&executable)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start C compiler");
    std::io::Write::write_all(compiler.stdin.as_mut().expect("compiler stdin"), source)
        .expect("write fixture source");
    let output = compiler
        .wait_with_output()
        .expect("compile session fixture");
    assert!(
        output.status.success(),
        "fixture compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

#[cfg(unix)]
fn wait_for_pid_pair(path: &Path) -> (u32, u32) {
    let attempts = if cfg!(coverage_nightly) { 2_000 } else { 500 };
    for _ in 0..attempts {
        if let Some(pair) = std::fs::read_to_string(path).ok().and_then(|contents| {
            let mut fields = contents.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        }) {
            return pair;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("provider fixture did not publish its pids")
}

#[cfg(unix)]
struct DaemonSessionCleanup {
    daemon_pid: u32,
    pid_file: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for DaemonSessionCleanup {
    fn drop(&mut self) {
        kill_process_group(libc::SIGKILL, self.daemon_pid as i32);
        if let Ok(contents) = std::fs::read_to_string(&self.pid_file)
            && let Some(provider) = contents
                .split_whitespace()
                .next()
                .and_then(|pid| pid.parse::<i32>().ok())
        {
            kill_process_group(libc::SIGKILL, provider);
        }
    }
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
