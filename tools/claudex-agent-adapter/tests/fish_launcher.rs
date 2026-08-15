use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fish_launcher_uses_the_shared_provider_config() {
    let home = shared_provider_fixture();
    let function = launcher_function();
    assert_shared_provider_default(&function, &home);
    assert_no_argument_launch(&function, &home);
    assert_settings_restore_modes(&function, &home);
    assert_explicit_model_restore(&function, &home);
    assert_explicit_override(&function, &home);
    assert_explicit_agent_is_preserved(&function, &home);
    assert_routing_marker_is_scoped_to_claudex(&function, &home);
    assert_user_local_adapter_forwards_hard_timeout(&function, &home);
    assert_local_defaults(&function, &home);
    assert_hostname_defaults_prefer_scoped_file(&function, &home);
    assert_hostname_disabled_models_config_is_exported(&function, &home);
}

#[test]
fn fish_launcher_defaults_to_pi_and_preserves_interface_overrides() {
    let home = shared_provider_fixture();
    let function = launcher_function();
    for (command, expected) in [
        ("claudex prompt-text", "pi"),
        ("claudex --provider-interface pi prompt-text", "pi"),
        ("claudex --provider-interface direct prompt-text", "direct"),
    ] {
        let arguments = run_fish_launcher(&function, &home, command);
        let (adapter, claude) = arguments
            .split_once("\n--\n")
            .expect("adapter and Claude arguments");
        assert!(
            adapter.ends_with(&format!("--provider-interface\n{expected}")),
            "adapter arguments: {arguments}"
        );
        assert!(!claude.contains("--provider-interface"));
        assert_eq!(claude, "prompt-text\n");
    }

    let environment = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex prompt-text", function.display()),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_PROVIDER_INTERFACE", "direct")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run environment provider interface");
    assert!(environment.status.success());
    let environment = String::from_utf8(environment.stdout).expect("UTF-8 adapter arguments");
    assert!(environment.contains("--provider-interface\ndirect\n--\nprompt-text\n"));

    let cli_override = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex --provider-interface pi prompt-text",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_PROVIDER_INTERFACE", "direct")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run CLI provider interface override");
    assert!(cli_override.status.success());
    let cli_override = String::from_utf8(cli_override.stdout).expect("UTF-8 adapter arguments");
    assert!(cli_override.contains("--provider-interface\npi\n--\nprompt-text\n"));

    for (arguments, expected) in [
        (
            "--provider-interface",
            "claudex: --provider-interface requires a value\n",
        ),
        (
            "--provider-interface invalid",
            "claudex: provider interface must be `pi` or `direct`\n",
        ),
        (
            "--provider-interface pi --provider-interface direct",
            "claudex: --provider-interface must not be repeated\n",
        ),
    ] {
        let output = Command::new("fish")
            .args([
                "-c",
                &format!("source '{}'; claudex {arguments}", function.display()),
            ])
            .env("HOME", home.path())
            .env_remove("CLAUDEX_PROVIDER_CONFIG")
            .env_remove("CLAUDEX_PROVIDER_INTERFACE")
            .output()
            .expect("run invalid provider interface");
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8(output.stderr)
                .expect("UTF-8 provider interface error")
                .ends_with(expected)
        );
    }

    let invalid_environment = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex prompt-text", function.display()),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_PROVIDER_INTERFACE", "invalid")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run invalid environment provider interface");
    assert_eq!(invalid_environment.status.code(), Some(2));
    assert!(
        String::from_utf8(invalid_environment.stderr)
            .expect("UTF-8 provider interface error")
            .ends_with("claudex: provider interface must be `pi` or `direct`\n")
    );
}

#[test]
fn hot_swap_wrappers_default_to_pi_and_preserve_overrides() {
    let home = shared_provider_fixture();
    let fish = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.config/fish/functions/claudex-hot-swap.fish");
    let posix =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/claudex-hot-swap");
    write_fake_curl(home.path());

    let default = run_hot_swap(&fish, &posix, home.path(), None, &[]);
    assert_hot_swap_interface(&default, "pi");

    let environment = run_hot_swap(&fish, &posix, home.path(), Some("direct"), &[]);
    assert_hot_swap_interface(&environment, "direct");

    let cli = run_hot_swap(
        &fish,
        &posix,
        home.path(),
        Some("direct"),
        &["--provider-interface", "pi"],
    );
    assert_hot_swap_interface(&cli, "pi");

    for (arguments, expected) in [
        (
            &["--provider-interface"] as &[&str],
            "claudex-hot-swap: --provider-interface requires a value\n",
        ),
        (
            &["--provider-interface", "invalid"],
            "claudex-hot-swap: provider interface must be `pi` or `direct`\n",
        ),
        (
            &[
                "--provider-interface",
                "pi",
                "--provider-interface",
                "direct",
            ],
            "claudex-hot-swap: --provider-interface must not be repeated\n",
        ),
    ] {
        assert_hot_swap_error(&fish, home.path(), arguments, expected);
        assert_hot_swap_error(&posix, home.path(), arguments, expected);
    }
}

fn write_fake_curl(home: &std::path::Path) {
    let curl = home.join(".local/bin/curl");
    fs::write(
        &curl,
        "#!/bin/sh\nprintf '%s\\n' '{\"build_id\":\"test\",\"pid\":1}'\n",
    )
    .expect("fake curl");
    let mut permissions = fs::metadata(&curl)
        .expect("fake curl metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&curl, permissions).expect("executable fake curl");
}

fn assert_hot_swap_interface(output: &str, expected: &str) {
    let flattened = output.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains(&format!("--provider-interface {expected}")),
        "hot-swap arguments: {output}"
    );
}

fn hot_swap_fish_command(function: &std::path::Path, arguments: &[&str]) -> String {
    let quoted = arguments.join(" ");
    format!("source '{}'; claudex-hot-swap {quoted}", function.display())
}

fn assert_hot_swap_error(
    program: &std::path::Path,
    home: &std::path::Path,
    arguments: &[&str],
    expected: &str,
) {
    let is_posix = program
        .file_name()
        .is_some_and(|name| name == "claudex-hot-swap");
    let output = if is_posix {
        let mut args = vec![program.to_string_lossy().into_owned()];
        args.extend(arguments.iter().map(|value| (*value).to_owned()));
        Command::new("sh")
            .args(args)
            .env("HOME", home)
            .env_remove("CLAUDEX_PROVIDER_INTERFACE")
            .output()
            .expect("run posix hot-swap error")
    } else {
        Command::new("fish")
            .args(["-c", &hot_swap_fish_command(program, arguments)])
            .env("HOME", home)
            .env_remove("CLAUDEX_PROVIDER_INTERFACE")
            .output()
            .expect("run fish hot-swap error")
    };
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .expect("UTF-8 hot-swap error")
            .ends_with(expected)
    );
}

fn run_hot_swap(
    fish: &std::path::Path,
    posix: &std::path::Path,
    home: &std::path::Path,
    environment: Option<&str>,
    arguments: &[&str],
) -> String {
    let mut combined = String::new();
    for (program, args) in [
        (
            "fish",
            vec!["-c".to_owned(), hot_swap_fish_command(fish, arguments)],
        ),
        ("sh", {
            let mut args = vec![posix.to_string_lossy().into_owned()];
            args.extend(arguments.iter().map(|value| (*value).to_owned()));
            args
        }),
    ] {
        let mut command = Command::new(program);
        command
            .args(args)
            .env("HOME", home)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    home.join(".local/bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env_remove("CLAUDEX_PROVIDER_CONFIG");
        match environment {
            Some(value) => {
                command.env("CLAUDEX_PROVIDER_INTERFACE", value);
            }
            None => {
                command.env_remove("CLAUDEX_PROVIDER_INTERFACE");
            }
        }
        let output = command.output().expect("run hot-swap wrapper");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        combined.push_str(&String::from_utf8(output.stdout).expect("UTF-8 hot-swap arguments"));
    }
    combined
}

#[test]
fn fish_launcher_keeps_command_tools_available_for_new_and_resumed_sessions() {
    let home = shared_provider_fixture();
    let function = launcher_function();

    let fresh = run_fish_launcher(&function, &home, "claudex command-tool-smoke");
    assert_command_tools_are_not_filtered(&fresh);
    assert!(fresh.ends_with("--\ncommand-tool-smoke\n"));
    assert_no_implicit_agent(&fresh);

    let resumed = run_fish_launcher(
        &function,
        &home,
        "claudex --resume retained-session --allowedTools 'Bash(git *),Bash(gh *)' resume-command-tool-smoke",
    );
    assert!(resumed.ends_with(
        "--\n--resume\nretained-session\n--allowedTools\nBash(git *),Bash(gh *)\nresume-command-tool-smoke\n"
    ));
    assert_no_implicit_agent(&resumed);
    let (adapter_arguments, _) = resumed
        .split_once("\n--\n")
        .expect("adapter and Claude arguments");
    assert!(
        !adapter_arguments
            .lines()
            .any(|argument| argument == "--model")
    );
    assert!(
        adapter_arguments
            .lines()
            .any(|argument| argument == "--inherit-claude-model")
    );

    let explicit = run_fish_launcher(
        &function,
        &home,
        "claudex --resume retained-session --tools Bash,Read explicit-tool-smoke",
    );
    assert!(explicit.contains("--tools\nBash,Read\n"));
}

fn run_fish_launcher(
    function: &std::path::Path,
    home: &tempfile::TempDir,
    command: &str,
) -> String {
    let output = run_fish_launcher_output(function, home, command, None);
    String::from_utf8(output.stdout).expect("UTF-8 adapter arguments")
}

fn run_fish_launcher_output(
    function: &std::path::Path,
    home: &tempfile::TempDir,
    command: &str,
    explicit_model: Option<&str>,
) -> std::process::Output {
    let mut process = Command::new("fish");
    process
        .args(["-c", &format!("source '{}'; {command}", function.display())])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .env_remove("CLAUDEX_PROVIDER_INTERFACE");
    if let Some(model) = explicit_model {
        process.env("CLAUDEX_MODEL", model);
    } else {
        process.env_remove("CLAUDEX_MODEL");
    }
    let output = process.output().expect("run fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn assert_command_tools_are_not_filtered(arguments: &str) {
    // New and resumed sessions inherit Claude Code's normal tool set. Claudex
    // restores missing schemas inside the adapter without changing CLI flags.
    for forbidden in ["--disallowedTools\n", "--disallowed-tools\n"] {
        assert!(
            !arguments.contains(forbidden),
            "launcher unexpectedly filters command tools with {forbidden:?}: {arguments}"
        );
    }
}

fn assert_no_implicit_agent(arguments: &str) {
    assert!(
        !arguments.contains("--agent\n"),
        "the default launcher must not force an orchestrator agent: {arguments}"
    );
}

fn shared_provider_fixture() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("temporary launcher home");
    fs::create_dir_all(home.path().join(".config/claudex")).expect("provider config directory");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::create_dir_all(home.path().join(".claude")).expect("settings directory");
    fs::write(
        home.path().join(".config/claudex/providers.json"),
        "{\"version\":1,\"mainProviders\":[\"p\"],\"providers\":[{\"id\":\"p\",\"agent\":\"worker\",\"defaultModel\":\"model\",\"subagentModel\":\"worker-model\",\"effort\":\"high\",\"backend\":\"codex-app-server\"}],\"fallback\":{\"agent\":\"fallback\",\"model\":\"sonnet\",\"effort\":\"high\"}}",
    )
    .expect("Grok config");
    fs::write(
        home.path().join(".claude/settings.json"),
        "{\"model\":\"sonnet[1m]\",\"effortLevel\":\"high\"}",
    )
    .expect("temporary settings file");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nprintf 'CLAUDEX_ACTIVE=%s\\n' \"${CLAUDEX_ACTIVE:-}\"\nprintf 'CLAUDEX_MAIN_MODEL=%s\\n' \"${CLAUDEX_MAIN_MODEL:-}\"\nprintf 'CLAUDEX_MAIN_MODEL_KNOWN=%s\\n' \"${CLAUDEX_MAIN_MODEL_KNOWN:-}\"\nprintf 'CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS=%s\\n' \"${CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS:-}\"\nprintf 'CLAUDE_CONFIG_DIR=%s\\n' \"${CLAUDE_CONFIG_DIR:-}\"\nprintf 'CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=%s\\n' \"${CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY:-}\"\nprintf 'CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=%s\\n' \"${CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS:-}\"\nprintf 'CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=%s\\n' \"${CLAUDE_CODE_ALWAYS_ENABLE_EFFORT:-}\"\nprintf '%s\\n' \"$@\"\n",
    )
    .expect("fake adapter");
    let mut permissions = fs::metadata(&adapter)
        .expect("fake adapter metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("executable fake adapter");
    home
}

fn launcher_function() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.config/fish/functions/claudex.fish")
}

fn assert_user_local_adapter_forwards_hard_timeout(
    function: &std::path::Path,
    home: &tempfile::TempDir,
) {
    let output = Command::new("fish")
        .args([
            "-c",
            &format!("source '{}'; claudex timeout-smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS", "17")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .env_remove("CLAUDEX_MODEL")
        .output()
        .expect("run user-local adapter timeout smoke");
    assert!(output.status.success());
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 timeout output");
    assert!(arguments.contains("CLAUDEX_SUBAGENT_HARD_TIMEOUT_SECONDS=17\n"));
    assert!(arguments.ends_with("--\ntimeout-smoke\n"));
}

fn assert_shared_provider_default(function: &std::path::Path, home: &tempfile::TempDir) {
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
    assert_shared_provider_args(&arguments);
    assert_shared_provider_settings(home, &output.stderr);
}

fn assert_shared_provider_args(arguments: &str) {
    assert!(arguments.contains("--provider-config\n"));
    assert!(arguments.contains("CLAUDEX_ACTIVE=1\n"));
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL=sonnet[1m]\n"));
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL_KNOWN=1\n"));
    assert!(
        arguments
            .lines()
            .any(|line| line.starts_with("CLAUDE_CONFIG_DIR=")
                && line.contains(".config/claudex/claude-config"))
    );
    assert!(arguments.contains("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1\n"));
    assert!(arguments.contains("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=40\n"));
    assert!(arguments.contains(".config/claudex/providers.json\n"));
    assert!(!arguments.contains("--model\n"));
    assert!(arguments.contains("--provider-interface\npi\n"));
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.contains("--subscription-max-processes\n20\n"));
    assert!(!arguments.contains("--allowedTools\nWebSearch,WebFetch\n"));
    assert!(arguments.ends_with("--\nsmoke\n"));
    assert_no_implicit_agent(arguments);
}

fn assert_shared_provider_settings(home: &tempfile::TempDir, stderr: &[u8]) {
    // Shared plain-claude settings must stay native; isolation seeds a separate tree.
    let shared_settings = fs::read_to_string(home.path().join(".claude/settings.json"))
        .expect("shared settings after launch");
    assert!(shared_settings.contains("\"model\":\"sonnet[1m]\""));
    let isolated_settings = fs::read_to_string(
        home.path()
            .join(".config/claudex/claude-config/settings.json"),
    )
    .expect("isolated settings after launch");
    assert!(isolated_settings.contains("\"model\": \"sonnet[1m]\""));
    assert!(
        String::from_utf8_lossy(stderr)
            .contains("current sonnet[1m], high; request model authoritative")
    );
}

fn assert_settings_restore_modes(function: &std::path::Path, home: &tempfile::TempDir) {
    let cases = [
        (
            "long resume",
            "claudex --resume retained-session resume-smoke",
            "--\n--resume\nretained-session\nresume-smoke\n",
        ),
        (
            "joined resume",
            "claudex --resume=retained-session resume-equals-smoke",
            "--\n--resume=retained-session\nresume-equals-smoke\n",
        ),
        (
            "short resume",
            "claudex -r retained-session short-resume-smoke",
            "--\n-r\nretained-session\nshort-resume-smoke\n",
        ),
        (
            "long continue",
            "claudex --continue continue-smoke",
            "--\n--continue\ncontinue-smoke\n",
        ),
        (
            "short continue",
            "claudex -c short-continue-smoke",
            "--\n-c\nshort-continue-smoke\n",
        ),
    ];

    for (label, command, expected_tail) in cases {
        let output = run_fish_launcher_output(function, home, command, None);
        let arguments = String::from_utf8(output.stdout).expect("UTF-8 restore arguments");
        let (adapter_arguments, _) = arguments
            .split_once("\n--\n")
            .expect("adapter and Claude arguments");
        assert!(
            !adapter_arguments
                .lines()
                .any(|argument| argument == "--model"),
            "{label}: {arguments}"
        );
        assert!(
            adapter_arguments
                .lines()
                .any(|argument| argument == "--inherit-claude-model"),
            "{label}: {arguments}"
        );
        assert!(
            arguments.contains("CLAUDEX_MAIN_MODEL=\n"),
            "{label}: {arguments}"
        );
        assert!(
            arguments.contains("CLAUDEX_MAIN_MODEL_KNOWN=0\n"),
            "{label}: {arguments}"
        );
        assert!(arguments.ends_with(expected_tail), "{label}: {arguments}");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("resumed orchestration"),
            "{label}: {diagnostic}"
        );
        assert!(
            diagnostic.contains(
                "current model restored by Claude Code and unknown to launcher; request model authoritative"
            ),
            "{label}: {diagnostic}"
        );
    }
}

fn assert_explicit_model_restore(function: &std::path::Path, home: &tempfile::TempDir) {
    let output = run_fish_launcher_output(
        function,
        home,
        "claudex --resume retained-session explicit-resume-smoke",
        Some("vendor-model"),
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 explicit restore arguments");
    let (adapter_arguments, _) = arguments
        .split_once("\n--\n")
        .expect("adapter and Claude arguments");
    assert!(adapter_arguments.contains("--model\nvendor-model\n"));
    assert!(!adapter_arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL=vendor-model\n"));
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL_KNOWN=1\n"));
    assert!(
        arguments
            .ends_with("--\n--effort\nmedium\n--resume\nretained-session\nexplicit-resume-smoke\n"),
        "{arguments}"
    );
}

fn assert_explicit_override(function: &std::path::Path, home: &tempfile::TempDir) {
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
    assert!(!arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.contains("--effort\nmedium\n"));
    assert!(!arguments.contains("--allowedTools\nWebSearch,WebFetch\n"));
    assert!(arguments.ends_with("--\n--effort\nmedium\noverride-smoke\n"));
    assert_no_implicit_agent(&arguments);
}

fn assert_local_defaults(function: &std::path::Path, home: &tempfile::TempDir) {
    fs::write(
        home.path().join(".config/claudex/defaults.local.json"),
        "{\"version\":1,\"source\":\"explicit\",\"model\":\"gpt-local\",\"effort\":\"low\"}",
    )
    .expect("explicit defaults");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex local-default-smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_EFFORT")
        .output()
        .expect("run explicit defaults fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 explicit defaults arguments");
    assert!(arguments.contains("--model\ngpt-local\n"));
    assert!(arguments.contains("--effort\nlow\n"));
    assert!(!arguments.contains("--inherit-claude-model\n"));

    fs::write(
        home.path().join(".config/claudex/defaults.local.json"),
        "{\"version\":1,\"source\":\"unsupported\",\"model\":\"gpt-local\",\"effort\":\"low\"}",
    )
    .expect("invalid defaults");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex invalid-default-smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_EFFORT")
        .output()
        .expect("run invalid defaults fish launcher");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("source must be `explicit` or `settings`")
    );
}

fn short_hostname() -> String {
    let output = Command::new("hostname")
        .arg("-s")
        .output()
        .expect("hostname -s");
    assert!(output.status.success(), "hostname -s failed");
    String::from_utf8(output.stdout)
        .expect("hostname utf-8")
        .trim()
        .to_owned()
}

fn assert_hostname_defaults_prefer_scoped_file(
    function: &std::path::Path,
    home: &tempfile::TempDir,
) {
    let hostname = short_hostname();
    fs::write(
        home.path().join(".config/claudex/defaults.local.json"),
        "{\"version\":1,\"model\":\"shared-local\",\"effort\":\"low\"}",
    )
    .expect("shared local defaults");
    fs::write(
        home.path()
            .join(format!(".config/claudex/defaults.{hostname}.local.json")),
        "{\"version\":1,\"model\":\"hostname-local\",\"effort\":\"max\"}",
    )
    .expect("hostname defaults");
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex hostname-default-smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_EFFORT")
        .env_remove("CLAUDEX_DEFAULTS_CONFIG")
        .output()
        .expect("run hostname defaults fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 hostname defaults arguments");
    assert!(arguments.contains("--model\nhostname-local\n"));
    assert!(arguments.contains("--effort\nmax\n"));
    assert!(!arguments.contains("--model\nshared-local\n"));
}

fn assert_hostname_disabled_models_config_is_exported(
    function: &std::path::Path,
    home: &tempfile::TempDir,
) {
    let hostname = short_hostname();
    let disabled_path = home.path().join(format!(
        ".config/claudex/disabled-subagent-models.{hostname}.local.json"
    ));
    fs::write(
        &disabled_path,
        r#"{"version":1,"disabledModels":["hostname-disabled"]}"#,
    )
    .expect("hostname disabled models");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nprintf 'DISABLED_CONFIG=%s\\n' \"${CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG:-}\"\nprintf '%s\\n' \"$@\"\n",
    )
    .expect("rewrite fake adapter");
    let mut permissions = fs::metadata(&adapter)
        .expect("fake adapter metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("executable fake adapter");

    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex disabled-config-smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG")
        .output()
        .expect("run disabled-config fish launcher");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 disabled-config arguments");
    assert!(
        stdout.contains(&format!("DISABLED_CONFIG={}\n", disabled_path.display())),
        "{stdout}"
    );
}

#[test]
fn fish_launcher_uses_claude_settings_model_and_effort_when_available() {
    let home = tempfile::tempdir().expect("temporary settings launcher home");
    fs::create_dir_all(home.path().join(".local/bin")).expect("adapter directory");
    fs::create_dir_all(home.path().join(".claude")).expect("settings directory");
    fs::create_dir_all(home.path().join(".config/claudex")).expect("provider config directory");
    fs::write(
        home.path().join(".claude/settings.json"),
        "{\"model\":\"sonnet[1m]\",\"effortLevel\":\"high\"}",
    )
    .expect("fixture settings");
    fs::write(
        home.path().join(".config/claudex/providers.json"),
        "{\"version\":1,\"mainProviders\":[\"p\"],\"providers\":[{\"id\":\"p\",\"agent\":\"worker\",\"defaultModel\":\"provider-model\",\"subagentModel\":\"worker-model\",\"effort\":\"high\",\"backend\":\"codex-app-server\"}],\"fallback\":{\"agent\":\"fallback\",\"model\":\"sonnet\",\"effort\":\"high\"}}",
    )
    .expect("fixture provider config");
    let adapter = home.path().join(".local/bin/claudex-agent-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nprintf 'CLAUDEX_MAIN_MODEL=%s\\n' \"${CLAUDEX_MAIN_MODEL:-}\"\nprintf 'CLAUDEX_MAIN_MODEL_KNOWN=%s\\n' \"${CLAUDEX_MAIN_MODEL_KNOWN:-}\"\nprintf 'CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=%s\\n' \"${CLAUDE_CODE_ALWAYS_ENABLE_EFFORT:-}\"\nprintf '%s\\n' \"$@\"\n",
    )
    .expect("fake adapter");
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
            &format!("source '{}'; claudex settings-smoke", function.display()),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_PROVIDER_CONFIG")
        .output()
        .expect("run fish launcher with settings");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 settings launcher arguments");
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL=sonnet[1m]\n"));
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL_KNOWN=1\n"));
    assert!(!arguments.contains("--model\n"));
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.contains(&format!(
        "--provider-config\n{}\n",
        home.path().join(".config/claudex/providers.json").display()
    )));
    assert!(arguments.ends_with("--\nsettings-smoke\n"));
    assert_no_implicit_agent(&arguments);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("current sonnet[1m], high; request model authoritative")
    );
}

#[test]
fn fish_config_sets_the_plain_claude_subagent_limit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config =
        fs::read_to_string(root.join(".config/fish/config.fish")).expect("fish configuration");
    assert!(
        config
            .lines()
            .any(|line| line.contains("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"))
    );
    assert!(
        config
            .lines()
            .any(|line| line.contains("or set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS 40"))
    );
    assert!(
        config.lines().any(|line| {
            line.contains(
            "set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; and set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"
        )
        }) || config.lines().any(|line| line.contains(
            "set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; or set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"
        ))
    );
}

fn assert_explicit_agent_is_preserved(function: &std::path::Path, home: &tempfile::TempDir) {
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex --agent custom-subagent smoke",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .output()
        .expect("run explicit-agent fish launcher");
    assert!(output.status.success());
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 explicit-agent arguments");
    assert_eq!(arguments.matches("--agent\n").count(), 1);
    assert!(arguments.ends_with("--\n--agent\ncustom-subagent\nsmoke\n"));
}

fn assert_routing_marker_is_scoped_to_claudex(
    function: &std::path::Path,
    home: &tempfile::TempDir,
) {
    let output = Command::new("fish")
        .args([
            "-c",
            &format!(
                "source '{}'; claudex marker-smoke; if set -q CLAUDEX_ACTIVE; echo leaked; end",
                function.display()
            ),
        ])
        .env("HOME", home.path())
        .env_remove("CLAUDEX_ACTIVE")
        .output()
        .expect("run routing marker smoke");
    assert!(output.status.success());
    let arguments = String::from_utf8(output.stdout).expect("UTF-8 marker arguments");
    assert!(!arguments.contains("leaked"));
}

#[test]
fn subagent_routes_and_fallback_match_worker_definitions() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".config/claudex/providers.json")).expect("provider config"),
    )
    .expect("valid provider config");
    let configured = config["providers"]
        .as_array()
        .expect("configured providers")
        .iter()
        .map(|provider| {
            (
                &provider["agent"],
                provider
                    .get("subagentModel")
                    .unwrap_or(&provider["defaultModel"]),
                &provider["effort"],
            )
        })
        .chain(std::iter::once((
            &config["fallback"]["agent"],
            &config["fallback"]["model"],
            &config["fallback"]["effort"],
        )));
    for (agent, model, effort) in configured {
        let name = format!("{}.md", agent.as_str().expect("worker agent"));
        let model = model.as_str().expect("worker model");
        let effort = effort.as_str().expect("worker effort");
        let definition = fs::read_to_string(root.join(".claude/agents").join(&name))
            .expect("provider worker definition");
        for expected in [format!("model: {model}"), format!("effort: {effort}")] {
            assert!(
                definition.lines().any(|line| line == expected),
                "{name} must match the shared provider route: {expected}"
            );
        }
    }

    assert_qwen_runtime_is_bounded(&root, &config);
    assert_provider_children_skip_claude_session_hook(&root);
}

#[test]
fn every_non_advisor_subagent_inherits_the_main_tool_and_permission_context() {
    let agents = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude/agents");
    for entry in fs::read_dir(agents).expect("Claude agent definitions") {
        let path = entry.expect("agent directory entry").path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("md") {
            continue;
        }
        let file_name = path.file_name().and_then(std::ffi::OsStr::to_str);
        if file_name == Some("custom-advisor.md") {
            continue;
        }
        let definition = fs::read_to_string(&path).expect("Claude agent definition");
        let frontmatter = definition
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .collect::<Vec<_>>();
        for restricted_field in ["tools:", "disallowedTools:", "permissionMode:"] {
            assert!(
                !frontmatter
                    .iter()
                    .any(|line| line.starts_with(restricted_field)),
                "{} must inherit instead of setting {restricted_field}",
                path.display()
            );
        }
        assert!(
            definition.contains("main session's complete tool set and permission context"),
            "{} must state the permission inheritance contract",
            path.display()
        );
    }
}

#[test]
fn custom_advisor_inherits_tools_while_preserving_peer_advisory_scope() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.claude/agents/custom-advisor.md");
    let definition = fs::read_to_string(&path).expect("custom advisor definition");
    let frontmatter = definition
        .lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .collect::<Vec<_>>();
    for forbidden_field in ["tools:", "disallowedTools:", "permissionMode:"] {
        assert!(
            !frontmatter
                .iter()
                .any(|line| line.starts_with(forbidden_field)),
            "custom-advisor must inherit the complete tool set, not {forbidden_field}"
        );
    }
    assert!(
        definition.contains("main session's complete tool set and permission context"),
        "custom-advisor must document permission inheritance"
    );
    assert!(
        definition.contains("SendMessage") && definition.contains("strategic advisor"),
        "custom-advisor must preserve its peer-advisory role"
    );
}

fn assert_qwen_runtime_is_bounded(root: &std::path::Path, config: &serde_json::Value) {
    let qwen = config["providers"]
        .as_array()
        .expect("configured providers")
        .iter()
        .find(|provider| provider["id"] == "qwen")
        .expect("Qwen provider");
    assert_eq!(qwen["acp"]["program"], "/usr/bin/env");
    assert_eq!(
        qwen["acp"]["arguments"],
        serde_json::json!([
            "QWEN_WEB_FETCH_PROCESSING_TIMEOUT_MS=8000",
            "qwen",
            "--acp",
            "--approval-mode",
            "yolo",
            "--model",
            "{model}"
        ])
    );

    let agent = fs::read_to_string(root.join(".claude/agents/claudex-qwen.md"))
        .expect("Qwen worker definition");
    let policy_text = agent.split_whitespace().collect::<Vec<_>>().join(" ");
    for policy in [
        "more than one `web_fetch` in a tool batch",
        "more than two `web_fetch` calls per delegated task",
        "Never retry the same or a substantially equivalent URL",
    ] {
        assert!(
            policy_text.contains(policy),
            "missing Qwen research policy: {policy}"
        );
    }
}

fn assert_provider_children_skip_claude_session_hook(root: &std::path::Path) {
    let settings: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join(".claude/settings.json")).expect("Claude settings"),
    )
    .expect("valid Claude settings");
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .expect("SessionStart command");
    let hook = command.find("herdr-agent-state.sh").expect("Herdr hook");
    for guard in [
        "CLAUDEX_NONINTERACTIVE_CHILD",
        "CLAUDEX_GROK_ACP",
        "CLAUDEX_PROVIDER_ACP",
    ] {
        assert!(
            command.find(guard).is_some_and(|index| index < hook),
            "{guard} must be checked before the Herdr hook reads stdin"
        );
        let output = Command::new("sh")
            .args(["-c", command])
            .env(guard, "1")
            .env("HOME", "/path/that/must/not/be-read")
            .output()
            .expect("run guarded SessionStart hook");
        assert!(output.status.success());
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
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL=sonnet[1m]\n"));
    assert!(arguments.contains("CLAUDEX_MAIN_MODEL_KNOWN=1\n"));
    assert!(!arguments.contains("--model\n"));
    assert!(arguments.contains("--inherit-claude-model\n"));
    assert!(arguments.ends_with("--\n"));
    assert_no_implicit_agent(&arguments);
}
