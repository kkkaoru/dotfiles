use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_claudex-route-usage")
}

struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    poison: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new(with_config: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let poison = root.path().join("poison-bin");
        let marker = root.path().join("process-started");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&poison).unwrap();
        for name in ["hostname", "codexbar", "curl", "vm_stat", "sysctl"] {
            write_poison(&poison.join(name), &marker);
        }
        if with_config {
            write_config(&home);
        }
        Self {
            root,
            home,
            poison,
            marker,
        }
    }

    fn run(&self, stdin: &str) -> Output {
        self.run_with(stdin, &[], &[])
    }

    fn run_with(&self, stdin: &str, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
        let mut command = Command::new(binary());
        command
            .args(arguments)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.poison)
            .env("CLAUDEX_REPOSITORY_ROOT", self.root.path())
            .env("CLAUDEX_USAGE_CACHE_SECONDS", "300")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.envs(environment.iter().copied());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn run_routed(&self, stdin: &str, environment: &[(&str, &str)]) -> Output {
        let codexbar = self.poison.join("codexbar");
        let curl = self.poison.join("curl");
        self.run_with(
            stdin,
            &[
                "--codexbar-program",
                codexbar.to_str().unwrap(),
                "--curl-program",
                curl.to_str().unwrap(),
            ],
            environment,
        )
    }

    fn routed_command(&self) -> Command {
        let mut command = Command::new(binary());
        command
            .arg("--codexbar-program")
            .arg(self.poison.join("codexbar"))
            .arg("--curl-program")
            .arg(self.poison.join("curl"))
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.poison)
            .env("CLAUDEX_REPOSITORY_ROOT", self.root.path())
            .env("CLAUDEX_USAGE_CACHE_SECONDS", "300")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn assert_no_process(&self) {
        assert!(!self.marker.exists(), "poison executable was invoked");
    }

    fn install_codexbar_snapshot_source(&self) {
        let path = self.poison.join("codexbar");
        fs::write(
            &path,
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' ",
                "'[{\"provider\":\"codex\",\"usage\":{\"primary\":{\"usedPercent\":10},",
                "\"secondary\":{\"usedPercent\":20}}}]'\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn cache_path(&self) -> PathBuf {
        self.home.join(".cache/claudex/usage-routing.json")
    }

    fn hold_refresh_lock(&self) -> File {
        let path = self
            .cache_path()
            .with_file_name("usage-routing.refresh.lock");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .unwrap();
        assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) }, 0);
        file
    }

    fn set_provider_cooldown(&self, active: bool) {
        let path = self.home.join(".cache/claudex/provider-auth-cooldown.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        if !active {
            fs::remove_file(path).unwrap();
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "entries": {
                    "codex": {
                        "untilUnixSeconds": now + 600,
                        "recordedUnixSeconds": now,
                        "message": "429"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn wait_for_cache_newer_than(&self, minimum: f64) -> Value {
        let cache = self.cache_path();
        let lock = cache.with_file_name("usage-routing.refresh.lock");
        wait_for_refreshed_cache(&cache, &lock, minimum)
    }
}

fn wait_for_refreshed_cache(cache: &Path, lock: &Path, minimum: f64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = refreshed_cache(cache, lock, minimum) {
            return value;
        }
        assert!(Instant::now() < deadline, "refresh did not publish cache");
        std::thread::yield_now();
    }
}

fn refreshed_cache(cache: &Path, lock: &Path, minimum: f64) -> Option<Value> {
    let value = serde_json::from_slice::<Value>(&fs::read(cache).ok()?).ok()?;
    (value["created_at"]
        .as_f64()
        .is_some_and(|created| created > minimum)
        && lock_is_available(lock))
    .then_some(value)
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::yield_now();
    }
    path.exists()
}

fn lock_is_available(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    let acquired = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if acquired {
        let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    acquired
}

fn write_poison(path: &Path, marker: &Path) {
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\nexit 97\n",
        marker.display()
    );
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn write_gate_program(path: &Path, ready: &Path, release: &Path) {
    let body = format!(
        "#!/bin/sh\nprintf ready > '{}'\nread _ < '{}'\nprintf '%s\\n' \
         '[{{\"provider\":\"codex\",\"usage\":{{\"primary\":{{\"usedPercent\":10}}}}}}]'\n",
        ready.display(),
        release.display()
    );
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn write_config(home: &Path) {
    let directory = home.join(".config/claudex");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("providers.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "providers": [{
                "id": "gpt",
                "agent": "claudex-gpt",
                "defaultModel": "gpt-5.6-luna",
                "effort": "high",
                "backend": "codex-app-server",
                "usageProvider": "codex"
            }],
            "fallback": {
                "agent": "claudex-sonnet",
                "model": "claude-sonnet-5",
                "effort": "high"
            },
            "nativeWorkers": [{
                "agent": "claudex-sonnet",
                "model": "claude-sonnet-5",
                "effort": "high",
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        directory.join("disabled-subagent-models.json"),
        r#"{"version":1,"disabledModels":[]}"#,
    )
    .unwrap();
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn context(output: &Output) -> String {
    json_stdout(output)["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("routing context")
        .to_owned()
}

fn metadata(output: &Output) -> Value {
    let context = context(output);
    let prefix = "Claudex routing data (runtime metadata; values only):\\n";
    let encoded = context
        .split_once(prefix)
        .expect("routing metadata prefix")
        .1
        .split_once("\\nClaudex tool policy")
        .expect("routing metadata suffix")
        .0;
    serde_json::from_str(encoded).expect("routing metadata JSON")
}

#[test]
fn exact_prompt_only_incident_bypasses_all_io_and_processes() {
    let fixture = Fixture::new(false);
    let payload = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "prompt": concat!(
            "<task-notification>\n",
            "<task-id>a760b564e16f0c75b</task-id>\n",
            "<status>completed</status>\n",
            "<summary>Agent \"worker\" finished</summary>\n",
            "</task-notification>"
        )
    });
    let output = fixture.run(&serde_json::to_string(&payload).unwrap());
    assert_eq!(json_stdout(&output), serde_json::json!({}));
    assert!(output.stderr.is_empty());
    fixture.assert_no_process();
}

#[test]
fn malformed_hook_payloads_are_successful_and_process_free() {
    for raw in [
        "",
        "{broken",
        "[]",
        "null",
        "\"text\"",
        "{}",
        r#"{"prompt":42}"#,
        r#"{"prompt":""}"#,
        r#"{"prompt":"　\t"}"#,
        r#"{"prompt":null,"user_prompt":"<task-notification>done</task-notification>"}"#,
    ] {
        let fixture = Fixture::new(false);
        let output = fixture.run(raw);
        assert_eq!(json_stdout(&output), serde_json::json!({}), "input={raw}");
        assert!(output.stderr.is_empty(), "input={raw}");
        fixture.assert_no_process();
    }
}

#[test]
fn exact_agent_finished_display_notification_bypasses_routing() {
    let fixture = Fixture::new(false);
    let output = fixture.run(
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"Agent \"Run autonomous smoke\" finished · 18m 32s"}"#,
    );
    assert_eq!(json_stdout(&output), serde_json::json!({}));
    assert!(output.stderr.is_empty());
    fixture.assert_no_process();
}

#[test]
fn notification_with_trailing_human_prompt_routes_normally() {
    let fixture = Fixture::new(true);
    let _guard = fixture.hold_refresh_lock();
    let output = fixture.run_routed(
        r#"{"prompt":"<task-notification>done</task-notification>\nPlease continue"}"#,
        &[],
    );
    assert!(context(&output).contains("Claudex routing data"));
    fixture.assert_no_process();
}

#[test]
fn normal_cold_hook_is_useful_policy_safe_and_starts_no_external_process() {
    let fixture = Fixture::new(true);
    let _guard = fixture.hold_refresh_lock();
    let output = fixture.run_routed(r#"{"prompt":"Please continue the real task"}"#, &[]);
    let metadata = metadata(&output);
    let workers = metadata["selected_workers"].as_array().unwrap();
    assert_eq!(
        workers.len(),
        1,
        "fallback/native worker must be deduplicated"
    );
    assert_eq!(workers[0]["agent"], "claudex-sonnet");
    assert_eq!(workers[0]["model"], "claude-sonnet-5");
    assert_eq!(metadata["disabled_subagent_models"], serde_json::json!([]));
    fixture.assert_no_process();
}

#[test]
fn cold_fallback_never_reintroduces_a_disabled_native_fallback_model() {
    let fixture = Fixture::new(true);
    let _guard = fixture.hold_refresh_lock();
    let output = fixture.run_routed(
        r#"{"prompt":"Route this task"}"#,
        &[("CLAUDEX_DISABLED_SUBAGENT_MODELS", "claude-sonnet-5")],
    );
    let metadata = metadata(&output);
    assert_eq!(
        metadata["disabled_subagent_models"],
        serde_json::json!(["claude-sonnet-5"])
    );
    assert!(
        metadata["selected_workers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|worker| worker["model"] != "claude-sonnet-5")
    );
    fixture.assert_no_process();
}

#[test]
fn cold_and_stale_hooks_refresh_then_fresh_hook_starts_no_provider_process() {
    let fixture = Fixture::new(true);
    fixture.install_codexbar_snapshot_source();
    let prompt = r#"{"prompt":"Please continue the real task"}"#;

    let cold = fixture.run_routed(prompt, &[]);
    assert!(
        json_stdout(&cold)["hookSpecificOutput"]["additionalContext"].is_string(),
        "cold fallback remains immediately useful"
    );
    let mut cached = fixture.wait_for_cache_newer_than(0.0);

    cached["created_at"] = Value::from(1.0);
    fs::write(fixture.cache_path(), serde_json::to_vec(&cached).unwrap()).unwrap();
    let stale = fixture.run_routed(
        prompt,
        &[
            ("CLAUDEX_MAIN_MODEL", "claude-sonnet-5"),
            ("CLAUDEX_MAIN_MODEL_KNOWN", "1"),
        ],
    );
    let stale = metadata(&stale);
    assert_eq!(stale["sonnet_subagent_suppressed"], true);
    assert_eq!(stale["selected_workers"][0]["agent"], "claudex-gpt");
    assert!(
        stale["selected_workers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|worker| worker["model"] != "claude-sonnet-5")
    );
    fixture.wait_for_cache_newer_than(1.0);

    write_poison(&fixture.poison.join("codexbar"), &fixture.marker);
    let _ = fs::remove_file(&fixture.marker);
    let fresh = fixture.run_routed(prompt, &[]);
    assert!(json_stdout(&fresh)["hookSpecificOutput"]["additionalContext"].is_string());
    fixture.assert_no_process();
}

#[test]
fn cold_hook_exits_before_gated_refresh_worker_and_worker_releases_lock() {
    let fixture = Fixture::new(true);
    let ready = fixture.root.path().join("worker-ready");
    let release = fixture.root.path().join("worker-release.fifo");
    assert_eq!(
        unsafe {
            libc::mkfifo(
                std::ffi::CString::new(release.to_str().unwrap())
                    .unwrap()
                    .as_ptr(),
                0o600,
            )
        },
        0
    );
    write_gate_program(&fixture.poison.join("codexbar"), &ready, &release);

    let mut command = fixture.routed_command();
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"prompt":"Route this task"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(context(&output).contains("Claudex routing data"));

    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let ready_thread = std::thread::spawn(move || {
        let _ = ready_tx.send(wait_for_path(&ready, Duration::from_secs(5)));
    });
    assert!(ready_rx.recv_timeout(Duration::from_secs(6)).unwrap());
    ready_thread.join().unwrap();
    assert!(
        !fixture.cache_path().exists(),
        "gated worker published early"
    );

    File::options()
        .write(true)
        .open(&release)
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    let cached = fixture.wait_for_cache_newer_than(0.0);
    assert_eq!(cached["generation"], 1);
}

#[test]
fn cooldown_start_and_release_invalidate_the_snapshot_generation() {
    let fixture = Fixture::new(true);
    fixture.install_codexbar_snapshot_source();
    let prompt = r#"{"prompt":"Route this task"}"#;

    let _ = fixture.run_routed(prompt, &[]);
    let clear_cache = fixture.wait_for_cache_newer_than(0.0);
    let clear_created = clear_cache["created_at"].as_f64().unwrap();
    assert!(
        metadata(&fixture.run_routed(prompt, &[]))["selected_workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|worker| worker["model"] == "gpt-5.6-luna")
    );

    fixture.set_provider_cooldown(true);
    let cooling = metadata(&fixture.run_routed(prompt, &[]));
    assert_eq!(
        cooling["disabled_subagent_models"],
        serde_json::json!(["gpt-5.6-luna"])
    );
    assert!(
        cooling["selected_workers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|worker| worker["model"] != "gpt-5.6-luna")
    );
    fixture.wait_for_cache_newer_than(clear_created);

    fixture.set_provider_cooldown(false);
    let cooling_cache = fs::read(fixture.cache_path()).unwrap();
    let cooling_created = serde_json::from_slice::<Value>(&cooling_cache).unwrap()["created_at"]
        .as_f64()
        .unwrap();
    let _ = fixture.run_routed(prompt, &[]);
    fixture.wait_for_cache_newer_than(cooling_created);
    let recovered = metadata(&fixture.run_routed(prompt, &[]));
    assert!(
        recovered["selected_workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|worker| worker["model"] == "gpt-5.6-luna")
    );
    assert_eq!(recovered["disabled_subagent_models"], serde_json::json!([]));
}
