use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::{PermissionsExt, symlink},
    os::unix::process::CommandExt,
    process::Command,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use reqwest::Client;
use serde_json::Value;
use tempfile::TempDir;

const ACTIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const FILE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CONCURRENT_LAUNCHERS: usize = 4;
const DAEMON_START_MARKER: &str = "=== claudex-agent-adapter daemon start ===";
const REPLACEMENT_READY_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_READY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct TestProcess {
    pid: u32,
    ppid: u32,
    pgid: u32,
    command: String,
}

struct LauncherProcessCleanup {
    home_marker: String,
    ports: Vec<u16>,
}

impl LauncherProcessCleanup {
    fn new(home: &TempDir, port: u16) -> Self {
        Self {
            home_marker: home.path().display().to_string(),
            ports: vec![port],
        }
    }

    fn cleanup_now(&self) {
        let snapshot = test_process_snapshot();
        let current_pgid = snapshot
            .iter()
            .find(|process| process.pid == std::process::id())
            .map(|process| process.pgid);
        let mut owned = snapshot
            .iter()
            .filter(|process| self.is_owned_daemon(process) || self.is_owned_provider(process))
            .map(|process| process.pid)
            .collect::<Vec<_>>();
        extend_with_descendants(&snapshot, &mut owned);
        if owned.is_empty() {
            return;
        }

        let mut private_groups = snapshot
            .iter()
            .filter(|process| owned.contains(&process.pid))
            .map(|process| process.pgid)
            .filter(|pgid| Some(*pgid) != current_pgid)
            .collect::<Vec<_>>();
        private_groups.sort_unstable();
        private_groups.dedup();
        for pid in &owned {
            signal_test_process("TERM", *pid, false);
        }
        for pgid in &private_groups {
            signal_test_process("TERM", *pgid, true);
        }
        wait_for_test_processes_to_exit(&owned, Duration::from_secs(2));
        for pid in owned
            .iter()
            .copied()
            .filter(|pid| test_process_exists(*pid))
        {
            signal_test_process("KILL", pid, false);
        }
        for pgid in &private_groups {
            kill_existing_test_process_group(*pgid);
        }
        wait_for_test_processes_to_exit(&owned, Duration::from_secs(1));
    }

    fn is_owned_daemon(&self, process: &TestProcess) -> bool {
        let trusted_binary = process
            .command
            .starts_with(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
            || process
                .command
                .starts_with(env!("CARGO_BIN_EXE_stale-adapter-mock"))
            || process.command.starts_with(&self.home_marker);
        trusted_binary
            && self.ports.iter().any(|port| {
                process
                    .command
                    .contains(&format!("--listen 127.0.0.1:{port}"))
                    || process
                        .command
                        .contains(&format!("--listen 0.0.0.0:{port}"))
            })
    }

    fn is_owned_provider(&self, process: &TestProcess) -> bool {
        process.command.contains(&self.home_marker) && process.command.contains("codex-mock")
    }
}

impl Drop for LauncherProcessCleanup {
    fn drop(&mut self) {
        self.cleanup_now();
    }
}

fn test_process_snapshot() -> Vec<TestProcess> {
    let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,command="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(TestProcess {
                pid: fields.next()?.parse().ok()?,
                ppid: fields.next()?.parse().ok()?,
                pgid: fields.next()?.parse().ok()?,
                command: fields.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

fn extend_with_descendants(snapshot: &[TestProcess], owned: &mut Vec<u32>) {
    while let Some(descendants) = next_descendants(snapshot, owned) {
        owned.extend(descendants);
    }
}

fn next_descendants(snapshot: &[TestProcess], owned: &[u32]) -> Option<Vec<u32>> {
    let descendants = snapshot
        .iter()
        .filter(|process| owned.contains(&process.ppid) && !owned.contains(&process.pid))
        .map(|process| process.pid)
        .collect::<Vec<_>>();
    (!descendants.is_empty()).then_some(descendants)
}

fn signal_test_process(signal: &str, id: u32, group: bool) {
    let target = if group {
        format!("-{id}")
    } else {
        id.to_string()
    };
    let _status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn test_process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn test_process_group_exists(pgid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &format!("-{pgid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn kill_existing_test_process_group(pgid: u32) {
    if test_process_group_exists(pgid) {
        signal_test_process("KILL", pgid, true);
    }
}

fn wait_for_test_processes_to_exit(pids: &[u32], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && pids.iter().any(|pid| test_process_exists(*pid)) {
        thread::sleep(Duration::from_millis(20));
    }
}

async fn active_slow_response(client: Client, base_url: String) -> String {
    client
        .get(format!("{base_url}/slow"))
        .send()
        .await
        .expect("active response")
        .text()
        .await
        .expect("active response body")
}

fn spawn_ensure_worker(
    mut command: Command,
    barrier: Arc<Barrier>,
) -> thread::JoinHandle<std::process::Output> {
    thread::spawn(move || {
        barrier.wait();
        command.output().expect("run concurrent ensure")
    })
}

#[tokio::test]
async fn protocol_compatible_build_replacement_preserves_an_active_response() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let stale_binary = home.path().join("stale/claudex-agent-adapter");
    fs::create_dir_all(stale_binary.parent().expect("stale binary directory"))
        .expect("create stale binary directory");
    fs::copy(env!("CARGO_BIN_EXE_stale-adapter-mock"), &stale_binary)
        .expect("copy stale adapter fixture");
    fs::set_permissions(&stale_binary, fs::Permissions::from_mode(0o755))
        .expect("make stale adapter executable");
    let entered = home.path().join("slow-entered");
    let release = home.path().join("slow-release");
    let mut stale = Command::new(&stale_binary)
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .args(["--entered", entered.to_str().expect("entered path")])
        .args(["--release", release.to_str().expect("release path")])
        .spawn()
        .expect("start stale adapter fixture");
    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{port}");
    let stale_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("stale adapter pid");
    let slow = tokio::spawn(active_slow_response(client.clone(), base_url.clone()));
    wait_for_file(&entered).await;

    let mut ensure = ensure_command(&home, port, "20");
    let output = tokio::time::timeout(
        ACTIVE_REQUEST_TIMEOUT,
        tokio::task::spawn_blocking(move || ensure.output()),
    )
    .await
    .expect("replacement must become ready while the old response is active")
    .expect("ensure task")
    .expect("replace compatible stale build");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replacement_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("replacement adapter pid");
    assert_ne!(replacement_pid, stale_pid);
    assert!(!slow.is_finished());
    assert!(stale.try_wait().expect("inspect stale adapter").is_none());

    fs::write(&release, "release").expect("release active response");
    assert_eq!(slow.await.expect("active response task"), "complete");
    assert!(stale.wait().expect("reap stale adapter").success());
    drop(client);
    terminate_and_wait(replacement_pid).await;
}

#[tokio::test]
async fn concurrent_ensure_commands_start_exactly_one_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let barrier = Arc::new(Barrier::new(CONCURRENT_LAUNCHERS));
    let workers = (0..CONCURRENT_LAUNCHERS)
        .map(|_| ensure_command(&home, port, "20"))
        .map(|command| spawn_ensure_worker(command, Arc::clone(&barrier)))
        .collect::<Vec<_>>();
    for worker in workers {
        let output = worker.join().expect("concurrent ensure worker");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert_eq!(daemon_start_count(&home), 1);
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("concurrently launched daemon pid");
    drop(client);
    terminate_and_wait(pid).await;
}

#[tokio::test]
async fn ensure_running_starts_reuses_and_replaces_the_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let first = ensure_command(&home, port, "20")
        .output()
        .expect("run ensure command");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let base_url = String::from_utf8(first.stdout)
        .expect("base URL output")
        .trim()
        .to_owned();
    assert_eq!(base_url, format!("http://127.0.0.1:{port}"));

    let client = Client::new();
    let initial = health(&client, &base_url).await;
    assert_eq!(initial["subscription_max_processes"], 20);
    let first_pid = initial["pid"].as_u64().expect("initial daemon pid");

    let reused = ensure_command(&home, port, "20")
        .output()
        .expect("reuse ensure command");
    assert!(reused.status.success());
    assert_eq!(
        health(&client, &base_url).await["pid"].as_u64(),
        Some(first_pid)
    );

    let authenticated = ensure_command(&home, port, "20")
        .env("ANTHROPIC_AUTH_TOKEN", "changed-token")
        .output()
        .expect("replace daemon after token change");
    assert!(authenticated.status.success());
    let authenticated_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("authenticated daemon pid");
    assert_ne!(authenticated_pid, first_pid);

    let replaced = ensure_command(&home, port, "7")
        .env("ANTHROPIC_AUTH_TOKEN", "changed-token")
        .output()
        .expect("replace ensure command");
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    let changed = health(&client, &base_url).await;
    assert_eq!(changed["subscription_max_processes"], 7);
    let replacement_pid = changed["pid"].as_u64().expect("replacement daemon pid");
    assert_ne!(replacement_pid, authenticated_pid);
    drop(client);
    terminate_and_wait(replacement_pid).await;
}

#[tokio::test]
async fn hard_timeout_environment_is_normalized_and_participates_in_daemon_identity() {
    const CURRENT: &str = "CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS";
    const LEGACY: &str = "CLAUDEX_SUBAGENT_RESPONSE_TIMEOUT_SECONDS";

    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let conflict = ensure_command(&home, port, "20")
        .env(CURRENT, "7")
        .env(LEGACY, "8")
        .output()
        .expect("reject conflicting timeout aliases");
    assert_error(conflict, "conflicts");
    let zero = ensure_command(&home, port, "20")
        .env(CURRENT, "0")
        .output()
        .expect("reject zero timeout");
    assert_error(zero, "positive integer");

    let legacy = ensure_command(&home, port, "20")
        .env(LEGACY, "17")
        .output()
        .expect("start daemon through legacy timeout alias");
    assert!(legacy.status.success());
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("deprecated"));
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let first = health(&client, &base_url).await;
    assert_eq!(first["subagent_hard_timeout_seconds"], 17);
    let first_pid = first["pid"].as_u64().expect("legacy timeout daemon pid");
    let first_fingerprint = first["service_config_fingerprint"]
        .as_str()
        .expect("service config fingerprint")
        .to_owned();

    let matching = ensure_command(&home, port, "20")
        .env(CURRENT, "17")
        .env(LEGACY, "17")
        .output()
        .expect("reuse matching timeout aliases");
    assert!(matching.status.success());
    assert_eq!(health(&client, &base_url).await["pid"], first_pid);

    let changed = ensure_command(&home, port, "20")
        .env(CURRENT, "23")
        .output()
        .expect("replace daemon after timeout change");
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let changed = health(&client, &base_url).await;
    assert_eq!(changed["subagent_hard_timeout_seconds"], 23);
    assert_ne!(changed["pid"], first_pid);
    assert_ne!(changed["service_config_fingerprint"], first_fingerprint);

    let unset = ensure_command(&home, port, "20")
        .output()
        .expect("replace daemon after clearing timeout");
    assert!(
        unset.status.success(),
        "{}",
        String::from_utf8_lossy(&unset.stderr)
    );
    let unset = health(&client, &base_url).await;
    assert!(unset["subagent_hard_timeout_seconds"].is_null());
    let pid = unset["pid"].as_u64().expect("unbounded daemon pid");
    drop(client);
    terminate_and_wait(pid).await;
}

#[tokio::test]
async fn failed_isolated_preflight_leaves_the_current_daemon_running() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let started = ensure_command(&home, port, "20")
        .output()
        .expect("start current daemon");
    assert!(started.status.success());
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let current_pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("current daemon pid");

    let failed = ensure_command(&home, port, "20")
        .env("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS", "17")
        .env("CLAUDEX_CODEX_PROGRAM", "/definitely/missing/codex")
        .output()
        .expect("run failing isolated preflight");
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("preflight"));
    assert_eq!(health(&client, &base_url).await["pid"], current_pid);
    drop(client);
    terminate_and_wait(current_pid).await;
}

#[tokio::test]
async fn main_model_change_replaces_instead_of_reusing_the_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let first = ensure_model_command(&home, port, "main-model-a")
        .output()
        .expect("start daemon with first main model");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let initial = health(&client, &base_url).await;
    assert_eq!(initial["model"], "main-model-a");
    let initial_pid = initial["pid"].as_u64().expect("initial daemon pid");
    let initial_fingerprint = initial["service_config_fingerprint"]
        .as_str()
        .expect("initial service fingerprint")
        .to_owned();

    let replacement = ensure_model_command(&home, port, "main-model-b")
        .output()
        .expect("replace daemon after changing the main model");
    assert!(
        replacement.status.success(),
        "{}",
        String::from_utf8_lossy(&replacement.stderr)
    );
    let changed = health(&client, &base_url).await;
    assert_eq!(changed["model"], "main-model-b");
    assert_ne!(changed["pid"].as_u64(), Some(initial_pid));
    assert_ne!(changed["service_config_fingerprint"], initial_fingerprint);

    drop(client);
    terminate_and_wait(changed["pid"].as_u64().expect("replacement daemon pid")).await;
}

#[tokio::test]
async fn effective_provider_program_change_replaces_instead_of_reusing_the_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let first = ensure_command(&home, port, "20")
        .output()
        .expect("start daemon with the default provider program");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let initial = health(&client, &base_url).await;
    let initial_pid = initial["pid"].as_u64().expect("initial daemon pid");
    let initial_fingerprint = initial["service_config_fingerprint"]
        .as_str()
        .expect("initial service fingerprint")
        .to_owned();

    let alternate_program = home.path().join("alternate-codex");
    fs::copy(env!("CARGO_BIN_EXE_codex-mock"), &alternate_program)
        .expect("copy alternate provider program");
    fs::set_permissions(&alternate_program, fs::Permissions::from_mode(0o755))
        .expect("make alternate provider program executable");
    let replacement = ensure_command(&home, port, "20")
        .env("CLAUDEX_CODEX_PROGRAM", &alternate_program)
        .output()
        .expect("replace daemon after changing the provider program");
    assert!(
        replacement.status.success(),
        "{}",
        String::from_utf8_lossy(&replacement.stderr)
    );
    let changed = health(&client, &base_url).await;
    assert_ne!(changed["pid"].as_u64(), Some(initial_pid));
    assert_ne!(changed["service_config_fingerprint"], initial_fingerprint);

    drop(client);
    terminate_and_wait(changed["pid"].as_u64().expect("replacement daemon pid")).await;
}

#[tokio::test]
async fn health_exposes_only_an_opaque_recovery_generation() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let started = ensure_command(&home, port, "20")
        .output()
        .expect("start daemon with recovery metadata");
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let state = health(&client, &base_url).await;
    let generation = state["recovery_generation"]
        .as_str()
        .filter(|generation| !generation.is_empty())
        .expect("opaque recovery generation");
    assert!(!generation.contains('/'));
    assert!(!generation.contains('\\'));
    assert!(state.get("recovery_manifest").is_none());
    assert!(
        !state
            .to_string()
            .contains(&home.path().display().to_string())
    );

    drop(client);
    terminate_and_wait(state["pid"].as_u64().expect("daemon pid")).await;
}

#[tokio::test]
async fn invalid_recovery_manifest_blocks_handover_and_keeps_the_old_daemon_serving() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let started = ensure_command(&home, port, "20")
        .output()
        .expect("start current daemon");
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let current = health(&client, &base_url).await;
    let current_pid = current["pid"].as_u64().expect("current daemon pid");
    let generation = current["recovery_generation"]
        .as_str()
        .expect("current recovery generation");
    let manifest = recovery_manifest_path(&home, generation);

    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o644))
        .expect("make recovery manifest unsafe");
    let unsafe_permissions = ensure_command(&home, port, "20")
        .env("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS", "17")
        .output()
        .expect("reject unsafe recovery manifest permissions");
    assert!(!unsafe_permissions.status.success());
    assert!(String::from_utf8_lossy(&unsafe_permissions.stderr).contains("recovery"));
    assert_eq!(health(&client, &base_url).await["pid"], current_pid);

    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600))
        .expect("restore recovery manifest permissions");
    let saved_manifest = manifest.with_extension("saved");
    fs::rename(&manifest, &saved_manifest).expect("move recovery manifest aside");
    symlink(&saved_manifest, &manifest).expect("replace recovery manifest with symlink");
    let unsafe_symlink = ensure_command(&home, port, "20")
        .env("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS", "17")
        .output()
        .expect("reject recovery manifest symlink");
    assert!(!unsafe_symlink.status.success());
    assert!(String::from_utf8_lossy(&unsafe_symlink.stderr).contains("recovery"));
    assert_eq!(health(&client, &base_url).await["pid"], current_pid);

    fs::remove_file(&manifest).expect("remove recovery manifest symlink");
    fs::rename(&saved_manifest, &manifest).expect("restore recovery manifest");
    drop(client);
    terminate_and_wait(current_pid).await;
}

#[tokio::test]
async fn direct_new_generation_start_failure_restores_the_prior_recovery_generation() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let started = ensure_command(&home, port, "20")
        .output()
        .expect("start prior daemon generation");
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let prior = health(&client, &base_url).await;
    let prior_pid = prior["pid"].as_u64().expect("prior daemon pid");
    let prior_fingerprint = prior["service_config_fingerprint"]
        .as_str()
        .expect("prior service fingerprint")
        .to_owned();

    let probe_port = unused_port();
    let _probe_process_cleanup = LauncherProcessCleanup::new(&home, probe_port);
    let probe = ensure_command(&home, probe_port, "20")
        .env("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS", "17")
        .output()
        .expect("start desired configuration probe");
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let probe_url = format!("http://127.0.0.1:{probe_port}");
    let probe_state = health(&client, &probe_url).await;
    let desired_fingerprint = probe_state["service_config_fingerprint"]
        .as_str()
        .expect("desired service fingerprint");
    let build_id = probe_state["build_id"].as_str().expect("adapter build id");
    let poisoned_generation =
        recovery_generation_name(&format!("127.0.0.1:{port}"), build_id, desired_fingerprint);
    terminate_and_wait(probe_state["pid"].as_u64().expect("probe daemon pid")).await;

    let poisoned_manifest = recovery_manifest_path(&home, &poisoned_generation);
    fs::write(&poisoned_manifest, b"not a recovery manifest")
        .expect("poison desired generation manifest");
    fs::set_permissions(&poisoned_manifest, fs::Permissions::from_mode(0o600))
        .expect("make poisoned manifest private");

    let failed_update = ensure_command(&home, port, "20")
        .env("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS", "17")
        .output()
        .expect("attempt direct new-generation start");
    assert!(!failed_update.status.success());
    assert!(String::from_utf8_lossy(&failed_update.stderr).contains("restored"));

    let recovered = health(&client, &base_url).await;
    let recovered_pid = recovered["pid"].as_u64().expect("recovered daemon pid");
    assert_ne!(recovered_pid, prior_pid);
    assert_eq!(recovered["service_config_fingerprint"], prior_fingerprint);
    assert_eq!(
        recovered["recovery_generation"],
        prior["recovery_generation"]
    );

    drop(client);
    terminate_and_wait(recovered_pid).await;
}

#[tokio::test]
async fn ensure_running_replaces_daemon_when_codex_config_changes() {
    let home = launcher_home();
    let codex_config = home.path().join(".codex/config.toml");
    fs::write(&codex_config, "[model_providers.sakana]\n# initial\n")
        .expect("write initial Codex config");
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);

    let first = ensure_command(&home, port, "20")
        .output()
        .expect("start daemon with initial Codex config");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let initial = health(&client, &base_url).await;
    let initial_pid = initial["pid"].as_u64().expect("initial daemon pid");
    let initial_fingerprint = initial["codex_config_fingerprint"]
        .as_str()
        .filter(|fingerprint| !fingerprint.is_empty())
        .expect("initial Codex config fingerprint")
        .to_owned();

    fs::write(&codex_config, "[model_providers.sakana]\n# changed\n").expect("change Codex config");
    let replaced = ensure_command(&home, port, "20")
        .output()
        .expect("replace daemon after Codex config change");
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );

    let changed = health(&client, &base_url).await;
    let replacement_pid = changed["pid"].as_u64().expect("replacement daemon pid");
    let replacement_fingerprint = changed["codex_config_fingerprint"]
        .as_str()
        .filter(|fingerprint| !fingerprint.is_empty())
        .expect("replacement Codex config fingerprint");
    assert_ne!(replacement_pid, initial_pid);
    assert_ne!(replacement_fingerprint, initial_fingerprint);

    drop(client);
    terminate_and_wait(replacement_pid).await;
}

#[tokio::test]
async fn ensure_running_replaces_the_renamed_legacy_daemon() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let current_binary = home.path().join("claudex-agent-adapter");
    let legacy_binary = home.path().join("claudex-app-server-adapter");
    for binary in [&current_binary, &legacy_binary] {
        fs::copy(env!("CARGO_BIN_EXE_claudex-agent-adapter"), binary)
            .expect("copy adapter under an installed name");
        fs::set_permissions(binary, fs::Permissions::from_mode(0o755))
            .expect("make copied adapter executable");
    }

    let mut legacy = Command::new(&legacy_binary)
        .args(["serve", "--model", "legacy-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"))
        .spawn()
        .expect("start renamed legacy daemon");
    let client = Client::new();
    let base_url = format!("http://127.0.0.1:{port}");
    let legacy_pid = health_with_deadline(
        &client,
        &base_url,
        Instant::now() + REPLACEMENT_READY_TIMEOUT,
    )
    .await["pid"]
        .as_u64()
        .expect("legacy daemon pid");
    // Do not retain an idle connection to the daemon while it drains during
    // handover. The replacement readiness probe below uses a fresh client.
    drop(client);

    let output = Command::new(&current_binary)
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args(["--subscription-timeout-minutes", "120"])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"))
        .output()
        .expect("replace renamed legacy daemon");
    if !output.status.success() {
        let _cleanup = legacy.kill();
        panic!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let replacement_client = Client::new();
    let replacement = replacement_health(&replacement_client, &base_url, legacy_pid).await;
    let _status = legacy.wait().expect("reap renamed legacy daemon");
    assert_eq!(replacement["model"], "test-main-model");
    assert_ne!(replacement["pid"].as_u64(), Some(legacy_pid));
    drop(replacement_client);
    terminate_and_wait(replacement["pid"].as_u64().expect("replacement daemon pid")).await;
}

#[tokio::test]
async fn ensure_running_replaces_an_unavailable_health_endpoint() {
    let home = launcher_home();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stale endpoint");
    let port = listener
        .local_addr()
        .expect("stale endpoint address")
        .port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let stale = thread::spawn(move || {
        serve_stale_health_after_releasing_listener(listener, unavailable_health())
    });
    let output = ensure_command(&home, port, "20")
        .output()
        .expect("replace unavailable endpoint");
    stale.join().expect("stale endpoint thread");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("replacement daemon pid");
    drop(client);
    terminate_and_wait(pid).await;
}

#[tokio::test]
async fn ensure_running_replaces_a_protocol_stale_foreign_endpoint() {
    let home = launcher_home();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stale endpoint");
    let port = listener
        .local_addr()
        .expect("stale endpoint address")
        .port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let body = format!(
        r#"{{"status":"ok","pid":{},"protocol_version":0,"build_id":"stale","model":"test-main-model","subscription_max_processes":20,"subscription_timeout_minutes":120}}"#,
        std::process::id()
    );
    let stale = thread::spawn(move || serve_stale_health(listener, 2, body));
    let output = ensure_command(&home, port, "20")
        .output()
        .expect("replace protocol-stale endpoint");
    stale.join().expect("stale endpoint thread");
    assert!(output.status.success());
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("replacement daemon pid");
    drop(client);
    terminate_and_wait(pid).await;
}

#[test]
fn ensure_running_rejects_non_loopback_without_a_real_token() {
    let home = launcher_home();
    let output = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", "0.0.0.0:8318"])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .output()
        .expect("run rejected ensure command");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ANTHROPIC_AUTH_TOKEN is required"));
}

#[tokio::test]
async fn ensure_running_connects_through_loopback_for_an_exposed_listener() {
    let home = launcher_home();
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);
    let output = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", &format!("0.0.0.0:{port}")])
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "real-token")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"))
        .output()
        .expect("run exposed adapter");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("http://127.0.0.1:{port}")
    );
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("exposed daemon pid");
    drop(client);
    terminate_and_wait(pid).await;
}

#[test]
fn ensure_running_reports_missing_or_invalid_environment() {
    let binary = env!("CARGO_BIN_EXE_claudex-agent-adapter");
    let missing_model = Command::new(binary)
        .arg("ensure")
        .output()
        .expect("run without model");
    assert_error(missing_model, "--model or --provider-config is required");

    let invalid_listen = Command::new(binary)
        .args([
            "ensure",
            "--model",
            "test-main-model",
            "--listen",
            "invalid",
        ])
        .output()
        .expect("run with invalid listener");
    assert_error(invalid_listen, "invalid --listen address");

    let missing_home = Command::new(binary)
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", "127.0.0.1:1"])
        .env_remove("HOME")
        .output()
        .expect("run without home");
    assert_error(missing_home, "HOME is required");

    let authenticated_exposed = Command::new(binary)
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", "0.0.0.0:8318"])
        .env("ANTHROPIC_AUTH_TOKEN", "real-token")
        .env_remove("HOME")
        .output()
        .expect("run exposed listener with authentication");
    assert_error(authenticated_exposed, "HOME is required");
}

#[tokio::test]
async fn run_claude_forwards_arguments_environment_stderr_and_status() {
    let home = launcher_home();
    let claude = prepare_claude_mock(&home);
    let port = unused_port();
    let _process_cleanup = LauncherProcessCleanup::new(&home, port);

    let path = format!(
        "{}:{}",
        home.path().display(),
        std::env::var("PATH").expect("PATH")
    );

    let output = run_mocked_claude(&home, port, &path);
    assert_eq!(output.status.code(), Some(23));
    assert_claude_wrapper_output(output);

    assert_inherited_launch(&home, port, &path);
    stop_wrapper_daemon(port).await;

    assert_duplicate_model_is_rejected(&home, port, &claude);
    assert_invalid_subagent_policy_is_rejected(&home, port, &claude);
}

#[tokio::test]
async fn process_cleanup_runs_during_unwind_and_releases_pid_group_and_port() {
    let home = launcher_home();
    let port = unused_port();
    let fallback_cleanup = LauncherProcessCleanup::new(&home, port);
    let started = ensure_command(&home, port, "20")
        .output()
        .expect("start daemon for cleanup regression test");
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("cleanup regression daemon pid") as u32;
    let pgid = test_process_snapshot()
        .into_iter()
        .find(|process| process.pid == pid)
        .map(|process| process.pgid)
        .expect("cleanup regression daemon process group");
    let provider_script = home.path().join("codex-mock");
    fs::write(&provider_script, "#!/bin/sh\nsleep 60\n").expect("write provider child fixture");
    fs::set_permissions(&provider_script, fs::Permissions::from_mode(0o755))
        .expect("make provider child fixture executable");
    let mut provider_child = Command::new(&provider_script)
        .env("HOME", home.path())
        .process_group(0)
        .spawn()
        .expect("start provider child fixture");
    let provider_pid = provider_child.id();
    let provider_pgid = test_process_snapshot()
        .into_iter()
        .find(|process| process.pid == provider_pid)
        .map(|process| process.pgid)
        .expect("provider child process group");
    drop(client);

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _panic_cleanup = LauncherProcessCleanup::new(&home, port);
        panic!("exercise launcher process cleanup during unwind");
    }));
    assert!(unwind.is_err());
    let _provider_status = provider_child.wait().expect("reap provider child fixture");
    assert!(
        !test_process_exists(pid),
        "daemon PID {pid} survived cleanup"
    );
    assert!(
        !test_process_group_exists(pgid),
        "daemon process group {pgid} survived cleanup"
    );
    assert!(
        !test_process_exists(provider_pid),
        "provider child PID {provider_pid} survived cleanup"
    );
    assert!(
        !test_process_group_exists(provider_pgid),
        "provider child process group {provider_pgid} survived cleanup"
    );
    let rebound = TcpListener::bind(("127.0.0.1", port)).expect("cleanup must release listen port");
    drop(rebound);
    drop(fallback_cleanup);
}

fn ensure_model_command(home: &TempDir, port: u16, model: &str) -> Command {
    let mut command = common_command(home, port, "20");
    command
        .args(["ensure", "--model", model])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args(["--subscription-timeout-minutes", "120"]);
    command
}

fn recovery_manifest_path(home: &TempDir, generation: &str) -> std::path::PathBuf {
    home.path()
        .join(".cache/claudex/recovery")
        .join(format!("manifest.{generation}.json"))
}

fn recovery_generation_name(listener: &str, build_id: &str, fingerprint: &str) -> String {
    let listener_hex = listener
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("v1-{listener_hex}-{build_id}-{fingerprint}")
}

fn prepare_claude_mock(home: &TempDir) -> std::path::PathBuf {
    let policy_directory = home.path().join(".config/claudex");
    fs::create_dir_all(&policy_directory).expect("create model policy directory");
    fs::write(
        policy_directory.join("disabled-subagent-models.json"),
        r#"{"version":1,"disabledModels":["configured-model"]}"#,
    )
    .expect("write model policy");
    let claude = home.path().join("claude");
    fs::write(
        &claude,
        r#"#!/bin/sh
printf 'args=%s\n' "$*"
printf 'base=%s effort=%s subagent=%s\n' "$ANTHROPIC_BASE_URL" "$CLAUDE_CODE_ALWAYS_ENABLE_EFFORT" "${CLAUDE_CODE_SUBAGENT_MODEL-unset}"
printf 'api_key=%s anthropic_model=%s bedrock=%s foundry=%s vertex=%s\n' \
    "${ANTHROPIC_API_KEY-unset}" "${ANTHROPIC_MODEL-unset}" \
    "${CLAUDE_CODE_USE_BEDROCK-unset}" "${CLAUDE_CODE_USE_FOUNDRY-unset}" \
    "${CLAUDE_CODE_USE_VERTEX-unset}"
printf 'custom_headers=%s\n' "$ANTHROPIC_CUSTOM_HEADERS"
printf 'resolved_models=%s\n' "$CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS"
printf "Advisor disabled — base model 'test-main-model' has no advisor rank\n" >&2
printf 'kept stderr\n' >&2
exit 23
"#,
    )
    .expect("write Claude mock");
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755))
        .expect("make Claude mock executable");
    claude
}

fn run_mocked_claude(home: &TempDir, port: u16, path: &str) -> std::process::Output {
    common_command(home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args(["--subscription-timeout-minutes", "120", "--"])
        .arg("--continue")
        .env("PATH", path)
        .env("CLAUDE_CODE_ALWAYS_ENABLE_EFFORT", "configured-by-fish")
        .env("CLAUDE_CODE_SUBAGENT_MODEL", "wrong-model")
        .env("ANTHROPIC_API_KEY", "must-not-leak")
        .env("ANTHROPIC_MODEL", "must-not-override")
        .env("CLAUDE_CODE_USE_BEDROCK", "1")
        .env("CLAUDE_CODE_USE_FOUNDRY", "1")
        .env("CLAUDE_CODE_USE_VERTEX", "1")
        .env(
            "ANTHROPIC_CUSTOM_HEADERS",
            "x-user-header: keep\nx-claudex-working-directory: forged\nx-claudex-disabled-subagent-models: forged",
        )
        .env(
            "CLAUDEX_DISABLED_SUBAGENT_MODELS",
            "grok-4.5,gpt-5.6-sol,grok-4.5",
        )
        .env("CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS", "forged")
        .output()
        .expect("run Claude wrapper")
}

fn assert_claude_wrapper_output(output: std::process::Output) {
    let stdout = String::from_utf8(output.stdout).expect("Claude stdout");
    assert!(stdout.contains("args=--model test-main-model --continue"));
    assert!(stdout.contains("effort=configured-by-fish subagent=unset"));
    assert!(
        stdout.contains(
            "api_key=unset anthropic_model=unset bedrock=unset foundry=unset vertex=unset"
        )
    );
    assert_terminal_policy_headers(&stdout);
    assert!(stdout.contains("resolved_models=configured-model,gpt-5.6-sol,grok-4.5"));
    let stderr = String::from_utf8(output.stderr).expect("Claude stderr");
    assert_eq!(stderr, "kept stderr\n");
}

async fn stop_wrapper_daemon(port: u16) {
    let base_url = format!("http://127.0.0.1:{port}");
    let client = Client::new();
    let pid = health(&client, &base_url).await["pid"]
        .as_u64()
        .expect("wrapper daemon pid");
    drop(client);
    terminate_and_wait(pid).await;
}

fn assert_duplicate_model_is_rejected(home: &TempDir, port: u16, claude: &std::path::Path) {
    let output = common_command(home, port, "20")
        .args([
            "launch",
            "--model",
            "test-main-model",
            "--",
            "--model",
            "other",
        ])
        .env("CLAUDEX_CLAUDE_PROGRAM", claude)
        .output()
        .expect("reject duplicate model");
    assert_error(output, "pass the main model to adapter option --model");
}

fn assert_terminal_policy_headers(output: &str) {
    assert!(output.contains("custom_headers=x-user-header: keep"));
    assert!(output.contains("x-claudex-working-directory:"));
    assert!(
        output
            .contains("x-claudex-disabled-subagent-models: configured-model,gpt-5.6-sol,grok-4.5")
    );
    assert!(!output.contains("forged"));
}

fn assert_invalid_subagent_policy_is_rejected(home: &TempDir, port: u16, claude: &std::path::Path) {
    let output = common_command(home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--", "--continue"])
        .env("CLAUDEX_CLAUDE_PROGRAM", claude)
        .env("CLAUDEX_DISABLED_SUBAGENT_MODELS", "model with spaces")
        .output()
        .expect("reject invalid terminal model policy");
    assert_error(output, "contains an invalid model ID");
}

fn assert_inherited_launch(home: &TempDir, port: u16, path: &str) {
    let inherited = common_command(home, port, "20")
        .args(["launch", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", "20"])
        .args([
            "--subscription-timeout-minutes",
            "120",
            "--inherit-claude-model",
            "--",
            "--agent",
            "claudex-orchestrator",
        ])
        .env("PATH", path)
        .output()
        .expect("run Claude wrapper with inherited model");
    assert_eq!(inherited.status.code(), Some(23));
    let inherited_stdout = String::from_utf8(inherited.stdout).expect("Claude stdout");
    assert!(inherited_stdout.contains("args=--agent claudex-orchestrator"));
    assert!(!inherited_stdout.contains("args=--model"));
}

fn ensure_command(home: &TempDir, port: u16, max_processes: &str) -> Command {
    let mut command = common_command(home, port, max_processes);
    command
        .args(["ensure", "--model", "test-main-model"])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--subscription-max-processes", max_processes])
        .args(["--subscription-timeout-minutes", "120"]);
    command
}

fn common_command(home: &TempDir, _port: u16, _max_processes: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"));
    command
        .env("HOME", home.path())
        .env("ANTHROPIC_AUTH_TOKEN", "claudex-local")
        .env("CLAUDEX_CODEX_PROGRAM", env!("CARGO_BIN_EXE_codex-mock"));
    command
}

fn launcher_home() -> TempDir {
    let home = tempfile::tempdir().expect("create launcher home");
    fs::create_dir(home.path().join(".codex")).expect("create Codex home");
    fs::write(
        home.path().join(".codex/auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write mock auth");
    home
}

async fn health(client: &Client, base_url: &str) -> Value {
    let deadline = Instant::now() + HEALTH_READY_TIMEOUT;
    loop {
        let Ok(response) = client.get(format!("{base_url}/health")).send().await else {
            sleep_until_health_deadline(deadline).await;
            continue;
        };
        let Ok(response) = response.error_for_status() else {
            sleep_until_health_deadline(deadline).await;
            continue;
        };
        let Ok(value) = response.json().await else {
            sleep_until_health_deadline(deadline).await;
            continue;
        };
        return value;
    }
}

async fn sleep_until_health_deadline(deadline: Instant) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    assert!(
        !remaining.is_zero(),
        "adapter health did not become readable"
    );
    tokio::time::sleep(Duration::from_millis(25).min(remaining)).await;
}

async fn replacement_health(client: &Client, base_url: &str, legacy_pid: u64) -> Value {
    let deadline = Instant::now() + REPLACEMENT_READY_TIMEOUT;
    loop {
        if let Some(value) = fetch_test_health(client, base_url).await
            && value["pid"].as_u64() != Some(legacy_pid)
        {
            return value;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25).min(remaining)).await;
    }
    panic!("replacement adapter health did not become readable")
}

async fn health_with_deadline(client: &Client, base_url: &str, deadline: Instant) -> Value {
    loop {
        if let Some(value) = fetch_test_health(client, base_url).await {
            return value;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25).min(remaining)).await;
    }
    panic!("adapter health did not become readable")
}

async fn fetch_test_health(client: &Client, base_url: &str) -> Option<Value> {
    let response = client
        .get(format!("{base_url}/health"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    response.json().await.ok()
}

fn terminate(pid: u64) {
    let status = Command::new("kill")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("terminate daemon");
    assert!(status.success());
}

async fn terminate_and_wait(pid: u64) {
    terminate(pid);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("adapter daemon did not exit");
}

fn process_is_alive(pid: u64) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(ACTIVE_REQUEST_TIMEOUT, async {
        while !path.exists() {
            tokio::time::sleep(FILE_POLL_INTERVAL).await;
        }
    })
    .await
    .expect("active request marker");
}

fn daemon_start_count(home: &TempDir) -> usize {
    fs::read_dir(home.path().join(".cache/claudex"))
        .expect("read launcher cache")
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .map(|contents| contents.matches(DAEMON_START_MARKER).count())
        .sum()
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("read ephemeral port")
        .port()
}

fn unavailable_health() -> String {
    r#"{"status":"unavailable","pid":null,"protocol_version":0,"build_id":"stale","model":"stale","subscription_max_processes":0,"subscription_timeout_minutes":0}"#.to_owned()
}

fn serve_stale_health(listener: TcpListener, responses: usize, body: String) {
    for _ in 0..responses {
        let (mut stream, _) = listener.accept().expect("accept health request");
        let mut request = [0_u8; 1024];
        let _bytes = stream.read(&mut request).expect("read health request");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write health response");
    }
}

fn serve_stale_health_after_releasing_listener(listener: TcpListener, body: String) {
    let (mut stream, _) = listener.accept().expect("accept health request");
    let mut request = [0_u8; 1024];
    let _bytes = stream.read(&mut request).expect("read health request");
    // `ensure` starts the replacement as soon as it receives this unavailable
    // health response. Release the port first so fixture scheduling cannot make
    // the replacement race a still-bound stale listener.
    drop(listener);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write health response");
}

fn assert_error(output: std::process::Output, expected: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
