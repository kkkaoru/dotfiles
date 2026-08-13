use std::os::unix::fs::PermissionsExt;

use super::signals::{
    capture_started_daemon_identity, process_group_is_alive, terminate_started_daemon_identity,
};
use super::*;

#[cfg(unix)]
use crate::launcher::wait_published;

#[cfg(unix)]
use std::{
    os::unix::process::CommandExt as _,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[cfg(unix)]
fn compile_orphaned_session_fixture(root: &Path) -> std::path::PathBuf {
    let executable = root.join("orphaned-session-fixture");
    let source = br#"
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>
int main(int argc, char **argv) {
    char tmp[4096];
    FILE *pids = NULL;
    pid_t child = -1;
    int i;
    if (argc != 2) return 2;
    for (i = 0; i < 50 && child < 0; i++) {
        child = fork();
        if (child >= 0) break;
        if (errno != EAGAIN) break;
        usleep(10000);
    }
    if (child < 0) return 3;
    if (child == 0) {
        if (setpgid(0, 0) != 0) _exit(4);
        for (;;) pause();
    }
    if (setpgid(child, child) != 0 && errno != EACCES) return 9;
    if (snprintf(tmp, sizeof tmp, "%s.tmp", argv[1]) >= (int)sizeof tmp) return 6;
    pids = fopen(tmp, "w");
    if (pids == NULL) return 5;
    if (fprintf(pids, "%d\n", child) < 0) return 7;
    fflush(pids);
    fsync(fileno(pids));
    fclose(pids);
    if (rename(tmp, argv[1]) != 0) return 8;
    for (;;) pause();
}
"#;
    let mut compiler = Command::new("cc");
    compiler
        .args(["-x", "c", "-o"])
        .arg(&executable)
        .arg("-")
        .env_remove("CFLAGS")
        .env_remove("CPPFLAGS")
        .env_remove("LDFLAGS")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut compiler = compiler.spawn().expect("start C compiler");
    std::io::Write::write_all(compiler.stdin.as_mut().expect("compiler stdin"), source)
        .expect("write fixture source");
    let output = compiler.wait_with_output().expect("compile orphan fixture");
    assert!(
        output.status.success(),
        "fixture compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("fixture executable");
    executable
}

#[cfg(unix)]
fn published_child_pid(path: &Path) -> Option<u32> {
    let pid = wait_published::readable(path)?.trim().parse().ok()?;
    process_is_alive(pid).then_some(pid)
}

#[cfg(unix)]
fn wait_for_child_pid(path: &Path, leader: &mut std::process::Child) -> u32 {
    wait_published::wait_until_published(
        path,
        Some(leader),
        "orphaned session fixture did not publish a child pid",
        published_child_pid,
    )
}

#[cfg(unix)]
fn spawn_orphaned_session_leader(fixture: &Path, pid_file: &Path) -> std::process::Child {
    let mut command = Command::new(fixture);
    command.arg(pid_file);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn session leader")
}

#[cfg(unix)]
#[test]
fn orphaned_session_cleanup_signals_remaining_process_groups() {
    let root = tempfile::tempdir().expect("orphaned session fixture");
    let fixture = compile_orphaned_session_fixture(root.path());
    let pid_file = root.path().join("child.pid");
    let mut leader = spawn_orphaned_session_leader(&fixture, &pid_file);
    let leader_pid = leader.id();
    let child_pid = wait_for_child_pid(&pid_file, &mut leader);
    let identity =
        capture_started_daemon_identity(leader_pid).expect("capture live session leader");
    kill_process(libc::SIGKILL, leader_pid);
    let _ = leader.wait();
    assert!(process_is_alive(child_pid));
    terminate_started_daemon_identity(&identity);
    for _ in 0..100 {
        if !process_is_alive(child_pid) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!process_is_alive(child_pid));
    assert!(capture_started_daemon_identity(leader_pid).is_none());
}

#[cfg(unix)]
#[test]
fn process_group_parser_skips_malformed_ps_rows() {
    if std::env::var_os("CLAUDEX_FAKE_PS_CHILD").is_some() {
        assert!(!process_group_is_alive(1));
        return;
    }
    let fixture = tempfile::tempdir().expect("fake ps fixture");
    let ps = fixture.path().join("ps");
    std::fs::write(&ps, "#!/bin/sh\necho '1 not-a-pgid S'\n").expect("fake ps");
    std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).expect("executable ps");
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(fixture.path().to_path_buf()).chain(std::env::split_paths(&original_path)),
    )
    .expect("PATH");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "launcher::daemon_process::signal_edges_tests::process_group_parser_skips_malformed_ps_rows",
        ])
        .env("CLAUDEX_FAKE_PS_CHILD", "1")
        .env("PATH", path)
        .status()
        .expect("run fake-ps child");
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn process_group_falls_back_when_ps_cannot_start() {
    if std::env::var_os("CLAUDEX_PS_MISSING_CHILD").is_some() {
        let _ = process_group_is_alive(i32::MAX);
        return;
    }
    let fixture = tempfile::tempdir().expect("missing ps fixture");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "launcher::daemon_process::signal_edges_tests::process_group_falls_back_when_ps_cannot_start",
        ])
        .env("CLAUDEX_PS_MISSING_CHILD", "1")
        .env("PATH", fixture.path())
        .status()
        .expect("run missing-ps child");
    assert!(status.success());
}
