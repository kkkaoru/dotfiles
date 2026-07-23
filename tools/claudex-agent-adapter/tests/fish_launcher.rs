use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fish_launcher_discovers_provider_models_without_pinning_them() {
    let home = tempfile::tempdir().expect("temporary launcher home");
    fs::create_dir_all(home.path().join(".codex")).expect("Codex config directory");
    fs::create_dir_all(home.path().join(".grok")).expect("Grok config directory");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::write(
        home.path().join(".codex/config.toml"),
        "model = \"gpt-5.6-test\"\n",
    )
    .expect("Codex config");
    fs::write(
        home.path().join(".grok/config.toml"),
        "[models]\ndefault = \"grok-4-test\"\n",
    )
    .expect("Grok config");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(&adapter, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").expect("fake adapter");
    let mut permissions = fs::metadata(&adapter)
        .expect("fake adapter metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("executable fake adapter");

    let function = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.config/fish/functions/claudex.fish");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_CODEX_MODEL")
        .env_remove("CLAUDEX_GROK_MODEL")
        .env_remove("CLAUDEX_BACKEND")
        .output()
        .expect("run fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 adapter arguments");
    assert!(arguments.contains("--model\ngpt-5.6-test\n"));
    assert!(arguments.contains("--backend-route\ngpt-5.6-test=codex-app-server\n"));
    assert!(arguments.contains("--backend-route\ngrok-4-test=grok-acp\n"));
    assert!(arguments.ends_with("--\nsmoke\n"));

    assert_no_argument_launch(&function, &home);

    let output = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex copilot-smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_MODEL", "gpt-5.6-test")
        .env("CLAUDEX_BACKEND", "copilot-acp")
        .output()
        .expect("run Copilot ACP fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 Copilot arguments");
    assert!(arguments.contains("--model\ngpt-5.6-test\n"));
    assert!(arguments.contains("--backend-route\ngpt-5.6-test=copilot-acp\n"));
    assert!(!arguments.contains("gpt-5.6-test=codex-app-server"));
    assert!(arguments.ends_with("--\ncopilot-smoke\n"));
}

fn assert_no_argument_launch(function: &std::path::Path, home: &tempfile::TempDir) {
    let output = Command::new("fish")
        .args(["-c", &format!("source '{}'; claudex", function.display())])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_CODEX_MODEL")
        .env_remove("CLAUDEX_GROK_MODEL")
        .env_remove("CLAUDEX_BACKEND")
        .output()
        .expect("run no-argument fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 adapter arguments");
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.ends_with("--\n--agent\nclaudex-orchestrator\n"));
}
