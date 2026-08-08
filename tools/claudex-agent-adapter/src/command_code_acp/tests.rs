use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

use agent_client_protocol as acp;
use tempfile::TempDir;

use super::{
    DEFAULT_MAX_TURNS, DEFAULT_MODEL, LaunchSpec, Options, ParsedLine, ProgressEvent,
    parse_stdout_line, process::run_turn, progress_to_updates, prompt_text,
};

fn spec_with_program(program: PathBuf) -> LaunchSpec {
    LaunchSpec {
        program,
        model: DEFAULT_MODEL.to_owned(),
        effort: Some("high".to_owned()),
        max_turns: 12,
        yolo: true,
        trust: true,
        skip_onboarding: true,
    }
}

fn write_executable(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, body).expect("write mock cmd");
    let mut permissions = fs::metadata(&path).expect("mock metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod mock cmd");
    path
}

#[test]
fn parse_options_defaults_and_overrides() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("COMMAND_CODE_CMD");
    let previous_home = std::env::var_os("HOME");
    unsafe { std::env::remove_var("COMMAND_CODE_CMD") };
    let isolated = TempDir::new().expect("isolated home");
    unsafe { std::env::set_var("HOME", isolated.path()) };
    let parsed = Options::parse(["--model", DEFAULT_MODEL, "--effort", "high"]).unwrap();
    assert_eq!(parsed.spec.model, DEFAULT_MODEL);
    assert_eq!(parsed.spec.effort.as_deref(), Some("high"));
    assert_eq!(parsed.spec.max_turns, DEFAULT_MAX_TURNS);
    assert_eq!(parsed.spec.program, PathBuf::from("cmd"));
    assert!(parsed.spec.yolo && parsed.spec.trust && parsed.spec.skip_onboarding);

    let parsed = Options::parse([
        "--model",
        "meta/muse-spark-1.2-contributor",
        "--cmd",
        "/tmp/cmd",
        "--max-turns",
        "4",
    ])
    .unwrap();
    assert_eq!(parsed.spec.program, PathBuf::from("/tmp/cmd"));
    assert_eq!(parsed.spec.max_turns, 4);

    assert!(Options::parse(["--unknown"]).is_err());
    assert!(Options::parse(["--model"]).is_err());
    assert!(Options::parse(["--model", ""]).is_err());
    assert!(Options::parse(["--max-turns", "0"]).is_err());
    assert!(Options::parse(["--max-turns", "nope"]).is_err());
    assert!(Options::parse(["--effort", "--high"]).is_err());
    match previous {
        Some(value) => unsafe { std::env::set_var("COMMAND_CODE_CMD", value) },
        None => unsafe { std::env::remove_var("COMMAND_CODE_CMD") },
    }
    match previous_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

#[test]
fn prefers_home_local_cmd_wrapper_when_present() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("COMMAND_CODE_CMD");
    let previous_home = std::env::var_os("HOME");
    unsafe { std::env::remove_var("COMMAND_CODE_CMD") };
    let home = TempDir::new().expect("wrapper home");
    let wrapper = write_executable(
        &{
            let bin = home.path().join(".local/bin");
            fs::create_dir_all(&bin).expect("wrapper bin");
            bin
        },
        "cmd",
        "#!/bin/sh\nexit 0\n",
    );
    unsafe { std::env::set_var("HOME", home.path()) };
    let parsed = Options::parse(["--model", DEFAULT_MODEL]).unwrap();
    assert_eq!(parsed.spec.program, wrapper);
    match previous {
        Some(value) => unsafe { std::env::set_var("COMMAND_CODE_CMD", value) },
        None => unsafe { std::env::remove_var("COMMAND_CODE_CMD") },
    }
    match previous_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

#[test]
fn command_code_env_overrides_program() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("COMMAND_CODE_CMD");
    unsafe { std::env::set_var("COMMAND_CODE_CMD", "/env/cmd") };
    let parsed = Options::parse(["--model", "meta/muse-spark-1.2-contributor"]).unwrap();
    assert_eq!(parsed.spec.program, PathBuf::from("/env/cmd"));
    match previous {
        Some(value) => unsafe { std::env::set_var("COMMAND_CODE_CMD", value) },
        None => unsafe { std::env::remove_var("COMMAND_CODE_CMD") },
    }
}

#[test]
fn argv_includes_headless_flags_and_resume() {
    let spec = spec_with_program(PathBuf::from("cmd"));
    let first = spec.argv("hello world", None);
    assert_eq!(
        first[..8],
        [
            "-p",
            "--output-format",
            "json",
            "--model",
            DEFAULT_MODEL,
            "--max-turns",
            "12",
            "--skip-onboarding"
        ]
    );
    assert!(first.contains(&"--yolo".to_owned()));
    assert!(first.contains(&"--trust".to_owned()));
    assert!(
        !first.contains(&"--effort".to_owned()),
        "Muse Spark rejects --effort; keep it ACP-side only: {first:?}"
    );
    assert_eq!(first.last().map(String::as_str), Some("hello world"));
    assert!(!first.contains(&"--resume".to_owned()));

    let resumed = spec.argv("follow up", Some("cc-session-1"));
    assert!(
        resumed
            .windows(2)
            .any(|window| window == ["--resume", "cc-session-1"])
    );
    assert_eq!(resumed.last().map(String::as_str), Some("follow up"));
    assert!(
        spec.argv("x", Some(""))
            .windows(2)
            .all(|window| window[0] != "--resume")
    );

    let minimal = LaunchSpec {
        program: PathBuf::from("cmd"),
        model: DEFAULT_MODEL.to_owned(),
        effort: None,
        max_turns: 1,
        yolo: false,
        trust: false,
        skip_onboarding: false,
    };
    let argv = minimal.argv("ping", None);
    assert!(!argv.iter().any(|flag| flag == "--yolo"));
    assert!(!argv.iter().any(|flag| flag == "--trust"));
    assert!(!argv.iter().any(|flag| flag == "--skip-onboarding"));
    assert!(!argv.iter().any(|flag| flag == "--effort"));
}

#[test]
fn parses_json_events_and_plain_progress() {
    assert_eq!(parse_stdout_line("  "), ParsedLine::Ignored);
    assert_eq!(
        parse_stdout_line("not json"),
        ParsedLine::Progress(ProgressEvent::Note("not json".to_owned()))
    );
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_running","toolCallId":"t1","toolName":"read_file","description":"A"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolStarted { id, name, .. }) if id == "t1" && name == "read_file"
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_completed","toolCallId":"t1","toolName":"read_file"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolCompleted { id, name }) if id == "t1" && name == "read_file"
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_failed","toolName":"shell","error":"nope"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolFailed { name, error, .. }) if name == "shell" && error.as_deref() == Some("nope")
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"retry","message":"again"}}"#),
        ParsedLine::Progress(ProgressEvent::Note(note)) if note == "retry: again"
    ));
}

#[test]
fn parses_event_aliases_and_error_shapes() {
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_start","toolName":"Grep"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolStarted { name, .. }) if name == "Grep"
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_done","toolCallId":"t9","toolName":"Grep"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolCompleted { id, name }) if id == "t9" && name == "Grep"
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_error","message":"denied"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolFailed { error, .. }) if error.as_deref() == Some("denied")
    ));
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":""}}"#),
        ParsedLine::Ignored
    );
    assert!(matches!(
        parse_stdout_line(r#"{"type":"status","message":"ok"}"#),
        ParsedLine::Progress(ProgressEvent::Note(_))
    ));
    let ParsedLine::Result(string_error) =
        parse_stdout_line(r#"{"type":"result","subtype":"error","finalText":"","error":"plain"}"#)
    else {
        panic!("expected string error");
    };
    assert_eq!(string_error.error.as_deref(), Some("plain"));
    let ParsedLine::Result(object_error) = parse_stdout_line(
        r#"{"type":"result","subtype":"error","finalText":"","error":{"code":10}}"#,
    ) else {
        panic!("expected object error");
    };
    assert!(
        object_error
            .error
            .as_deref()
            .is_some_and(|error| error.contains("code"))
    );
}

#[test]
fn parses_live_command_code_ndjson_without_flooding_tui() {
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"run_start","sessionId":"s"}}"#),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"text_delta","delta":"PONG"}}"#),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_end","text":""}}"#),
        ParsedLine::Ignored
    );
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_end","text":"planning"}}"#),
        ParsedLine::Progress(ProgressEvent::Note(note)) if note == "planning"
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"turn_start","turnNumber":1}}"#),
        ParsedLine::Progress(ProgressEvent::Note(note)) if note == "turn 1"
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"model_request_start","model":"meta/muse-spark-1.2-contributor"}}"#),
        ParsedLine::Progress(ProgressEvent::Note(note)) if note.contains("meta/muse-spark-1.2-contributor")
    ));
    let ParsedLine::Result(result) = parse_stdout_line(
        r#"{"type":"result","subtype":"success","sessionId":"166f0397-5c8a-4ac0-bb7c-8e7f3287227f","stopReason":"end_turn","finalText":"PONG"}"#,
    ) else {
        panic!("expected live result");
    };
    assert_eq!(
        result.session_id.as_deref(),
        Some("166f0397-5c8a-4ac0-bb7c-8e7f3287227f")
    );
    assert_eq!(result.final_text, "PONG");
}

#[test]
fn parses_result_lines_and_formats_messages() {
    let ParsedLine::Result(result) = parse_stdout_line(
        r#"{"type":"result","subtype":"error","sessionId":"s1","finalText":"","error":{"message":"boom"}}"#,
    ) else {
        panic!("expected result");
    };
    assert_eq!(result.session_id.as_deref(), Some("s1"));
    assert_eq!(result.error.as_deref(), Some("boom"));
    assert!(super::events::result_is_error(&result));
    assert_eq!(super::events::result_message(&result), "boom");
    let ok = super::events::TurnResult {
        subtype: "success".to_owned(),
        session_id: None,
        stop_reason: Some("end_turn".to_owned()),
        final_text: "done".to_owned(),
        error: None,
    };
    assert!(!super::events::result_is_error(&ok));
    assert_eq!(super::events::result_message(&ok), "done");
    let empty_error = super::events::TurnResult {
        subtype: "error".to_owned(),
        session_id: None,
        stop_reason: Some("error".to_owned()),
        final_text: String::new(),
        error: None,
    };
    assert!(super::events::result_message(&empty_error).contains("subtype `error`"));
}

#[test]
fn progress_updates_include_thought_and_tool_chrome() {
    let started = progress_to_updates(&ProgressEvent::Started {
        model: DEFAULT_MODEL.to_owned(),
        effort: None,
    });
    assert!(matches!(
        started[0],
        acp::SessionUpdate::AgentThoughtChunk(_)
    ));
    let running = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "t1".to_owned(),
        name: "read_file".to_owned(),
        description: Some("README.md".to_owned()),
    });
    assert!(matches!(
        running[0],
        acp::SessionUpdate::AgentThoughtChunk(_)
    ));
    assert!(matches!(running[1], acp::SessionUpdate::ToolCall(_)));
    let done = progress_to_updates(&ProgressEvent::ToolCompleted {
        id: "t1".to_owned(),
        name: "read_file".to_owned(),
    });
    assert!(matches!(done[1], acp::SessionUpdate::ToolCallUpdate(_)));
    let failed = progress_to_updates(&ProgressEvent::ToolFailed {
        id: "t2".to_owned(),
        name: "shell".to_owned(),
        error: Some("denied".to_owned()),
    });
    assert!(matches!(failed[1], acp::SessionUpdate::ToolCallUpdate(_)));
    let note = progress_to_updates(&ProgressEvent::Note("retry".to_owned()));
    assert!(matches!(note[0], acp::SessionUpdate::AgentThoughtChunk(_)));
    let other = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "5".to_owned(),
        name: "Planner".to_owned(),
        description: None,
    });
    let acp::SessionUpdate::ToolCall(call) = &other[1] else {
        panic!("other call");
    };
    assert_eq!(call.kind, acp::ToolKind::Other);
    let failed_no_error = progress_to_updates(&ProgressEvent::ToolFailed {
        id: "6".to_owned(),
        name: "shell".to_owned(),
        error: None,
    });
    assert!(matches!(
        failed_no_error[1],
        acp::SessionUpdate::ToolCallUpdate(_)
    ));
}

#[test]
fn prompt_text_joins_content_blocks() {
    let request = acp::PromptRequest::new(
        "session",
        vec![
            acp::ContentBlock::Text(acp::TextContent::new("first")),
            acp::ContentBlock::Text(acp::TextContent::new("second")),
        ],
    );
    assert_eq!(prompt_text(&request), "first\nsecond");
}

#[tokio::test]
async fn run_turn_parses_mock_cmd_success_and_resume_argv() {
    let root = TempDir::new().expect("mock cmd dir");
    let trace = root.path().join("trace.jsonl");
    let program = write_executable(
        root.path(),
        "cmd",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\ncat <<'EOF'\n{{\"type\":\"event\",\"event\":{{\"type\":\"tool_running\",\"toolCallId\":\"t1\",\"toolName\":\"read_file\",\"description\":\"README.md\"}}}}\n{{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-1\",\"stopReason\":\"end_turn\",\"finalText\":\"COMMAND_CODE_HEADLESS_OK\"}}\nEOF\n",
            trace.display()
        ),
    );
    let spec = spec_with_program(program);
    let first = run_turn(&spec, "hello", None).await.expect("first turn");
    assert_eq!(first.result.session_id.as_deref(), Some("cc-1"));
    assert_eq!(first.result.final_text, "COMMAND_CODE_HEADLESS_OK");
    assert!(first.progress.iter().any(
        |event| matches!(event, ProgressEvent::ToolStarted { name, .. } if name == "read_file")
    ));
    let _ = run_turn(&spec, "again", Some("cc-1"))
        .await
        .expect("resume turn");
    let recorded = fs::read_to_string(&trace).expect("read mock trace");
    assert!(recorded.contains("--resume cc-1"));
    assert!(recorded.contains("--output-format json"));
    assert!(recorded.contains(DEFAULT_MODEL));
}

#[tokio::test]
async fn run_turn_maps_auth_and_max_turn_exit_codes() {
    let root = TempDir::new().expect("exit mock dir");
    let auth = write_executable(root.path(), "auth", "#!/bin/sh\nexit 3\n");
    let outcome = run_turn(&spec_with_program(auth), "hi", None)
        .await
        .expect("auth failure still yields outcome");
    assert_eq!(outcome.exit_code, Some(3));
    assert!(
        outcome
            .result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not authenticated"))
    );

    let max_turns = write_executable(root.path(), "max", "#!/bin/sh\nexit 8\n");
    let outcome = run_turn(&spec_with_program(max_turns), "hi", None)
        .await
        .expect("max-turns outcome");
    assert!(
        outcome
            .result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("max-turns"))
    );

    let credits = write_executable(root.path(), "credits", "#!/bin/sh\nexit 10\n");
    let outcome = run_turn(&spec_with_program(credits), "hi", None)
        .await
        .expect("credits outcome");
    assert!(
        outcome
            .result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("insufficient credits"))
    );

    let unsupported_effort = write_executable(
        root.path(),
        "effort",
        "#!/bin/sh\necho 'Muse Spark 1.2 Contributor has no adjustable reasoning effort.' >&2\nexit 1\n",
    );
    let outcome = run_turn(&spec_with_program(unsupported_effort), "hi", None)
        .await
        .expect("stderr fallback");
    assert!(
        outcome
            .result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("no adjustable reasoning effort"))
    );

    let unknown = write_executable(root.path(), "unknown", "#!/bin/sh\nexit 42\n");
    let outcome = run_turn(&spec_with_program(unknown), "hi", None)
        .await
        .expect("unknown exit");
    assert!(
        outcome.result.error.as_deref().is_some_and(|error| error
            .contains("without a JSON result")
            && error.contains("exit 42"))
    );

    let silent_ok = write_executable(root.path(), "silent", "#!/bin/sh\nexit 0\n");
    let outcome = run_turn(&spec_with_program(silent_ok), "hi", None)
        .await
        .expect("silent success");
    assert_eq!(outcome.result.subtype, "success");
    assert!(outcome.result.final_text.is_empty());

    let ignored = write_executable(
        root.path(),
        "ignored",
        "#!/bin/sh\nprintf '\\n \\n'\nexit 0\n",
    );
    let outcome = run_turn(&spec_with_program(ignored), "hi", None)
        .await
        .expect("ignored blank lines");
    assert_eq!(outcome.result.subtype, "success");

    let missing = spec_with_program(root.path().join("missing-cmd"));
    assert!(
        run_turn(&missing, "hi", None)
            .await
            .expect_err("missing binary")
            .to_string()
            .contains("failed to spawn")
    );
}

#[test]
fn tool_kind_mapping_covers_common_command_code_names() {
    let read = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "1".to_owned(),
        name: "Grep".to_owned(),
        description: None,
    });
    let acp::SessionUpdate::ToolCall(call) = &read[1] else {
        panic!("tool call");
    };
    assert_eq!(call.kind, acp::ToolKind::Read);

    let edit = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "2".to_owned(),
        name: "WriteFile".to_owned(),
        description: None,
    });
    let acp::SessionUpdate::ToolCall(call) = &edit[1] else {
        panic!("edit call");
    };
    assert_eq!(call.kind, acp::ToolKind::Edit);

    let shell = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "3".to_owned(),
        name: "Bash".to_owned(),
        description: None,
    });
    let acp::SessionUpdate::ToolCall(call) = &shell[1] else {
        panic!("shell call");
    };
    assert_eq!(call.kind, acp::ToolKind::Execute);

    let search = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "4".to_owned(),
        name: "WebSearch".to_owned(),
        description: None,
    });
    let acp::SessionUpdate::ToolCall(call) = &search[1] else {
        panic!("search call");
    };
    assert_eq!(call.kind, acp::ToolKind::Search);
}
