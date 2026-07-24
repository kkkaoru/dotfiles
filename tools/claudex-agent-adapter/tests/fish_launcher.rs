use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fish_launcher_uses_the_shared_provider_config() {
    let home = tempfile::tempdir().expect("temporary launcher home");
    fs::create_dir_all(home.path().join(".config/claudex")).expect("provider config directory");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::write(
        home.path().join(".config/claudex/providers.json"),
        "{\"version\":1}",
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
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 adapter arguments");
    assert!(arguments.contains("--provider-config\n"));
    assert!(arguments.contains(".config/claudex/providers.json\n"));
    assert!(arguments.ends_with("--\nsmoke\n"));

    assert_no_argument_launch(&function, &home);

    let alternate = home.path().join("alternate-providers.json");
    fs::write(&alternate, "{\"version\":1}").expect("alternate config");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex override-smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_PROVIDER_CONFIG", &alternate)
        .env("CLAUDEX_MODEL", "vendor-model")
        .output()
        .expect("run Copilot ACP fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 override arguments");
    assert!(arguments.contains(&format!("--provider-config\n{}\n", alternate.display())));
    assert!(arguments.contains("--model\nvendor-model\n"));
    assert!(arguments.ends_with("--\noverride-smoke\n"));
}

#[test]
fn provider_workers_leave_model_selection_to_the_shared_config() {
    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/agents");
    for name in ["claudex-gpt.md", "claudex-grok.md"] {
        let definition = fs::read_to_string(agents.join(name)).expect("provider worker definition");
        assert!(
            definition.lines().any(|line| line == "model: inherit"),
            "{name} must not bypass config-driven model routing"
        );
    }
}

fn assert_no_argument_launch(function: &std::path::Path, home: &tempfile::TempDir) {
    let output = Command::new("fish")
        .args(["-c", &format!("source '{}'; claudex", function.display())])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run no-argument fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 adapter arguments");
    assert!(!arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.ends_with("--\n--agent\nclaudex-orchestrator\n"));
}
