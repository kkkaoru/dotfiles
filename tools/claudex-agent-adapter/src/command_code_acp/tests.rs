use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

use agent_client_protocol as acp;
use serde_json::json;
use tempfile::TempDir;

use super::events::turn_cancelled_updates;
use super::{
    DEFAULT_MAX_TURNS, DEFAULT_MODEL, LaunchSpec, Options, ParsedLine, ProgressCoalescer,
    ProgressEvent, parse_stdout_line,
    process::{run_turn, run_turn_emitting},
    progress_to_updates, prompt_text, remaining_final_message, slim_headless_prompt,
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
    let defaulted = Options::parse(Vec::<&str>::new()).unwrap();
    assert_eq!(defaulted.spec.model, DEFAULT_MODEL);
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
fn default_cmd_without_wrapper_uses_bare_cmd() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var_os("COMMAND_CODE_CMD");
    let previous_home = std::env::var_os("HOME");
    unsafe { std::env::remove_var("COMMAND_CODE_CMD") };
    let home = TempDir::new().expect("home without wrapper");
    unsafe { std::env::set_var("HOME", home.path()) };
    let parsed = Options::parse(["--model", DEFAULT_MODEL]).unwrap();
    assert_eq!(parsed.spec.program, PathBuf::from("cmd"));
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
    assert!(first.contains(&"--no-skills".to_owned()));
    assert!(first.contains(&"--no-session".to_owned()));
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
    assert!(resumed.contains(&"--no-skills".to_owned()));
    assert!(
        !resumed.contains(&"--no-session".to_owned()),
        "--no-session is incompatible with --resume: {resumed:?}"
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
    assert!(argv.iter().any(|flag| flag == "--no-skills"));
    assert!(argv.iter().any(|flag| flag == "--no-session"));
}

#[tokio::test]
async fn run_turn_writes_optional_command_code_trace() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let root = TempDir::new().expect("trace dir");
    let trace = root.path().join("cc-trace.json");
    let previous = std::env::var_os("CLAUDEX_COMMAND_CODE_TRACE");
    unsafe { std::env::set_var("CLAUDEX_COMMAND_CODE_TRACE", &trace) };
    let program = write_executable(
        root.path(),
        "ok",
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"finalText\":\"ok\"}'\n",
    );
    let outcome = run_turn_emitting(
        &spec_with_program(program.clone()),
        "traced-task",
        None,
        None,
        Some(root.path()),
    )
    .await
    .expect("traced turn");
    assert_eq!(outcome.result.final_text, "ok");
    let recorded = fs::read_to_string(&trace).expect("trace file");
    assert!(
        recorded.contains("traced-task") || recorded.contains("-p"),
        "{recorded}"
    );
    unsafe { std::env::set_var("CLAUDEX_COMMAND_CODE_TRACE", "   ") };
    run_turn(&spec_with_program(program), "blank-trace", None)
        .await
        .expect("blank trace env is ignored");
    match previous {
        Some(value) => unsafe { std::env::set_var("CLAUDEX_COMMAND_CODE_TRACE", value) },
        None => unsafe { std::env::remove_var("CLAUDEX_COMMAND_CODE_TRACE") },
    }
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
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_running","toolCallId":"w1","toolName":"web_search","query":"AVITA株式会社 公式"}}"#),
        ParsedLine::Progress(ProgressEvent::ToolStarted { name, description, .. }) if name == "web_search" && description.as_deref() == Some("AVITA株式会社 公式")
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_running","toolCallId":"w2","toolName":"web_search","arguments":{"query":"AVITA Inc funding"}}}"#),
        ParsedLine::Progress(ProgressEvent::ToolStarted { description, .. }) if description.as_deref() == Some("AVITA Inc funding")
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
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"text_delta","delta":"PONG"}}"#),
        ParsedLine::Progress(ProgressEvent::Message(note)) if note == "PONG"
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"message_update","text":"AVITA findings"}}"#
        ),
        ParsedLine::Progress(ProgressEvent::Message(note)) if note == "AVITA findings"
    ));
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"message_update"}}"#),
        ParsedLine::Ignored
    );
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"planning live"}}"#
        ),
        ParsedLine::Progress(ProgressEvent::Thought(note)) if note == "planning live"
    ));
    assert_eq!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"Thought for 15s"}}"#
        ),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_end","text":""}}"#),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_end","text":"planning"}}"#),
        ParsedLine::Ignored
    );
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_delta","text":"● 検索中: AVITA"}}"#),
        ParsedLine::Progress(ProgressEvent::Status(note)) if note.contains("検索中")
    ));
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_delta"}}"#),
        ParsedLine::Ignored
    );
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"起動: Command Code Muse Spark"}}"#
        ),
        ParsedLine::Ignored
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"実行中: web_search。次: ツール結果待ち"}}"#
        ),
        ParsedLine::Ignored
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"完了: web_search。次: 続きの調査または回答"}}"#
        ),
        ParsedLine::Ignored
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"失敗: web_search。次: 別手段"}}"#
        ),
        ParsedLine::Ignored
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"ターン1開始"}}"#
        ),
        ParsedLine::Ignored
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"実行中: still working"}}"#
        ),
        ParsedLine::Progress(ProgressEvent::Thought(_))
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"完了: web_search"}}"#
        ),
        ParsedLine::Progress(ProgressEvent::Thought(_))
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"失敗: web_search"}}"#
        ),
        ParsedLine::Progress(ProgressEvent::Thought(_))
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"thinking_delta","text":"ターン1 準備中"}}"#
        ),
        ParsedLine::Progress(ProgressEvent::Thought(_))
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_delta","text":"▶ searching"}}"#),
        ParsedLine::Progress(ProgressEvent::Status(note)) if note.starts_with('▶')
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_delta","text":"✓ done"}}"#),
        ParsedLine::Progress(ProgressEvent::Status(note)) if note.starts_with('✓')
    ));
    assert!(matches!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"thinking_delta","text":"✗ failed"}}"#),
        ParsedLine::Progress(ProgressEvent::Status(note)) if note.starts_with('✗')
    ));
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"api_retry"}}"#),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"tool_queued"}}"#),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(r#"{"type":"event","event":{"type":"turn_start","turnNumber":1}}"#),
        ParsedLine::Ignored
    );
    assert_eq!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"model_request_start","model":"meta/muse-spark-1.2-contributor"}}"#
        ),
        ParsedLine::Ignored
    );
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
    let error_field_only = super::events::TurnResult {
        subtype: "success".to_owned(),
        session_id: None,
        stop_reason: Some("end_turn".to_owned()),
        final_text: String::new(),
        error: Some("boom".to_owned()),
    };
    assert!(super::events::result_is_error(&error_field_only));
    assert_eq!(super::events::result_message(&error_field_only), "boom");
    let stop_reason_only = super::events::TurnResult {
        subtype: "success".to_owned(),
        session_id: None,
        stop_reason: Some("error".to_owned()),
        final_text: String::new(),
        error: None,
    };
    assert!(super::events::result_is_error(&stop_reason_only));
    let empty_error = super::events::TurnResult {
        subtype: "error".to_owned(),
        session_id: None,
        stop_reason: Some("error".to_owned()),
        final_text: String::new(),
        error: None,
    };
    assert!(super::events::result_message(&empty_error).contains("subtype `error`"));
}

fn first_tool_call(updates: &[acp::SessionUpdate]) -> &acp::ToolCall {
    updates
        .iter()
        .find_map(|update| match update {
            acp::SessionUpdate::ToolCall(call) => Some(call),
            _ => None,
        })
        .expect("tool call")
}

fn first_tool_update(updates: &[acp::SessionUpdate]) -> &acp::ToolCallUpdate {
    updates
        .iter()
        .find_map(|update| match update {
            acp::SessionUpdate::ToolCallUpdate(update) => Some(update),
            _ => None,
        })
        .expect("tool update")
}

fn rendered_messages(updates: &[acp::SessionUpdate]) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn rendered_thoughts(updates: &[acp::SessionUpdate]) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            acp::SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
                acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn tool_raw_input_uses_shared_provider_argument_keys() {
    use super::tool_chrome::tool_raw_input;
    assert_eq!(
        tool_raw_input("web_fetch", Some("https://avita.co.jp/")),
        json!({"url": "https://avita.co.jp/"})
    );
    assert_eq!(
        tool_raw_input("grep", Some("AVITA")),
        json!({"pattern": "AVITA"})
    );
    assert_eq!(
        tool_raw_input("shell", Some("ls src")),
        json!({"command": "ls src"})
    );
    assert_eq!(
        tool_raw_input("Glob", Some("**/*.rs")),
        json!({"path": "**/*.rs"})
    );
    assert_eq!(
        tool_raw_input("Notify", Some("plan")),
        json!({"description": "plan"})
    );
    assert_eq!(
        tool_raw_input("Read", Some("src/main.rs")),
        json!({"path": "src/main.rs"})
    );
    assert_eq!(
        tool_raw_input("Write", Some("out.md")),
        json!({"path": "out.md"})
    );
    assert_eq!(
        tool_raw_input("exec", Some("pwd")),
        json!({"command": "pwd"})
    );
    assert_eq!(
        tool_raw_input("web_search", Some("  AVITA  ")),
        json!({"query": "AVITA"})
    );
    assert_eq!(tool_raw_input("other", None), json!({}));
    assert_eq!(tool_raw_input("read_file", Some("   ")), json!({}));
}

#[test]
fn progress_updates_include_thought_and_tool_chrome() {
    let started = progress_to_updates(&ProgressEvent::Started {
        model: DEFAULT_MODEL.to_owned(),
        effort: None,
    });
    assert!(started.is_empty());
    let running = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "t1".to_owned(),
        name: "read_file".to_owned(),
        description: Some("README.md".to_owned()),
    });
    assert!(matches!(running[0], acp::SessionUpdate::ToolCall(_)));
    assert_eq!(&*first_tool_call(&running).tool_call_id.0, "t1");
    assert_eq!(first_tool_call(&running).title, "read_file");
    assert_eq!(
        first_tool_call(&running).raw_input.as_ref(),
        Some(&json!({"path": "README.md"}))
    );
    assert!(rendered_messages(&running).is_empty());
    let searching = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "t-search".to_owned(),
        name: "web_search".to_owned(),
        description: Some("AVITA株式会社".to_owned()),
    });
    assert_eq!(first_tool_call(&searching).title, "web_search");
    assert_eq!(
        first_tool_call(&searching).raw_input.as_ref(),
        Some(&json!({"query": "AVITA株式会社"}))
    );
    let done = progress_to_updates(&ProgressEvent::ToolCompleted {
        id: "t1".to_owned(),
        name: "read_file".to_owned(),
    });
    assert!(
        done.iter()
            .any(|update| matches!(update, acp::SessionUpdate::ToolCallUpdate(_)))
    );
    assert!(rendered_messages(&done).is_empty());
    let failed = progress_to_updates(&ProgressEvent::ToolFailed {
        id: "t2".to_owned(),
        name: "shell".to_owned(),
        error: Some("denied".to_owned()),
    });
    let failed_update = first_tool_update(&failed);
    assert_eq!(failed_update.fields.title.as_deref(), Some("shell"));
    assert_eq!(
        failed_update.fields.status,
        Some(acp::ToolCallStatus::Failed)
    );
    assert_eq!(
        failed_update.fields.raw_output.as_ref(),
        Some(&json!("denied"))
    );
    assert!(rendered_messages(&failed).is_empty());
    let failed_bare = progress_to_updates(&ProgressEvent::ToolFailed {
        id: "t3".to_owned(),
        name: "shell".to_owned(),
        error: None,
    });
    assert_eq!(first_tool_update(&failed_bare).fields.raw_output, None);
    let note = progress_to_updates(&ProgressEvent::Note("retry".to_owned()));
    assert!(note.is_empty());
    let message = progress_to_updates(&ProgressEvent::Message("AVITA report".to_owned()));
    assert!(matches!(
        message[0],
        acp::SessionUpdate::AgentMessageChunk(_)
    ));
    assert!(rendered_messages(&message).contains("AVITA report"));
    let status = progress_to_updates(&ProgressEvent::Status("検索中: AVITA".to_owned()));
    assert!(rendered_thoughts(&status).contains("検索中: AVITA"));
    assert!(rendered_messages(&status).is_empty());
    let thought = progress_to_updates(&ProgressEvent::Thought("planning".to_owned()));
    assert!(rendered_thoughts(&thought).contains("planning"));
    assert!(rendered_messages(&thought).is_empty());
    let thought_status = progress_to_updates(&ProgressEvent::Thought("● 検索中".to_owned()));
    assert!(rendered_thoughts(&thought_status).contains("● 検索中"));
    assert!(rendered_messages(&thought_status).is_empty());
    for prefix in ["▶ searching", "✓ done", "✗ failed"] {
        let updates = progress_to_updates(&ProgressEvent::Message(prefix.to_owned()));
        assert!(rendered_thoughts(&updates).contains(prefix), "{prefix}");
        assert!(rendered_messages(&updates).is_empty(), "{prefix}");
    }
    let canned = progress_to_updates(&ProgressEvent::Message(
        "● 実行中: web_search。次: ツール結果待ち".to_owned(),
    ));
    assert!(canned.is_empty());
    let canned_done = progress_to_updates(&ProgressEvent::Status(
        "完了: web_search。次: 続きの調査または回答".to_owned(),
    ));
    assert!(canned_done.is_empty());
    let other = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "5".to_owned(),
        name: "Planner".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&other).kind, acp::ToolKind::Other);
    let failed_no_error = progress_to_updates(&ProgressEvent::ToolFailed {
        id: "6".to_owned(),
        name: "shell".to_owned(),
        error: None,
    });
    assert!(
        failed_no_error
            .iter()
            .any(|update| matches!(update, acp::SessionUpdate::ToolCallUpdate(_)))
    );
}

#[test]
fn drops_canned_command_code_status_phrases() {
    for canned in [
        "次: タスク実行",
        "次: ツールまたは回答",
        "▶ 次: トークン待ち",
        "✓ 次: 別手段または報告",
        "✗ 次: 中断",
        "起動: Command Code Muse Spark",
        "失敗: web_search。次: 別手段",
        "ターン1開始",
        "モデル要求中: meta/muse-spark-1.2-contributor",
        "Thought for 15s",
        "Thought for 19s",
    ] {
        assert!(
            progress_to_updates(&ProgressEvent::Status(canned.to_owned())).is_empty(),
            "{canned}"
        );
    }
}

#[test]
fn cancel_updates_emit_visible_text_without_turn_chrome() {
    let updates = turn_cancelled_updates();
    assert!(rendered_messages(&updates).contains("Command Code cancelled"));
    assert!(
        updates
            .iter()
            .all(|update| !matches!(update, acp::SessionUpdate::ToolCallUpdate(_)))
    );
}

#[test]
fn coalesces_tiny_deltas_but_flushes_complete_phrases() {
    let mut coalescer = ProgressCoalescer::default();
    assert!(
        coalescer
            .push(ProgressEvent::Thought("ア".to_owned()))
            .is_empty()
    );
    assert!(
        coalescer
            .push(ProgressEvent::Thought("ビ".to_owned()))
            .is_empty()
    );
    let flushed = coalescer.push(ProgressEvent::Thought("TA INC report".to_owned()));
    assert_eq!(
        flushed,
        vec![ProgressEvent::Thought("アビTA INC report".to_owned())]
    );
    assert_eq!(
        coalescer.push(ProgressEvent::Message("調査完了。".to_owned())),
        vec![ProgressEvent::Message("調査完了。".to_owned())]
    );
    let long = "a".repeat(80);
    assert_eq!(
        coalescer.push(ProgressEvent::Thought(long.clone())),
        vec![ProgressEvent::Thought(long)]
    );
    assert!(
        coalescer
            .push(ProgressEvent::Message("L".to_owned()))
            .is_empty()
    );
    assert_eq!(
        coalescer.push(ProgressEvent::Message("IVE_DELTA".to_owned())),
        vec![ProgressEvent::Message("LIVE_DELTA".to_owned())]
    );
    coalescer.push(ProgressEvent::Thought("pending".to_owned()));
    assert_eq!(
        coalescer.push(ProgressEvent::ToolStarted {
            id: "t1".to_owned(),
            name: "web_search".to_owned(),
            description: None,
        }),
        vec![
            ProgressEvent::Thought("pending".to_owned()),
            ProgressEvent::ToolStarted {
                id: "t1".to_owned(),
                name: "web_search".to_owned(),
                description: None,
            }
        ]
    );
    coalescer.push(ProgressEvent::Message("partial".to_owned()));
    assert_eq!(
        coalescer.finish(),
        vec![ProgressEvent::Message("partial".to_owned())]
    );
}

#[test]
fn remaining_final_message_skips_already_streamed_answer() {
    assert_eq!(
        remaining_final_message("REPORT", ""),
        Some("REPORT".to_owned())
    );
    assert_eq!(remaining_final_message("REPORT", "REPORT"), None);
    assert_eq!(remaining_final_message("REPORT", "● 起動\nREPORT"), None);
    assert_eq!(
        remaining_final_message("hello world", "hello"),
        Some("world".to_owned())
    );
    assert_eq!(remaining_final_message("", "anything"), None);
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
    let rendered = prompt_text(&request);
    assert!(rendered.starts_with("first\nsecond"));
    assert!(rendered.contains("Do not greet"));
    assert!(rendered.contains("phase update is not the answer"));
    assert!(rendered.contains("only between tool calls"));
    assert!(rendered.contains("native thinking/? elapsed and web cards"));
    assert!(rendered.contains("Status:"));
    assert!(!rendered.contains("▶ name: query/path/url"));
    assert!(rendered.contains("assistant text"));
    assert!(!rendered.contains("current/next/blocker"));
}

#[test]
fn recognizes_command_code_model_aliases() {
    for model in [
        "meta/muse-spark-1.2-contributor",
        "Muse-Spark",
        "command-code",
        "command-code/muse",
    ] {
        assert!(
            super::is_command_code_model(model),
            "{model} should route as Command Code"
        );
    }
    for model in ["grok-4.5", "gpt-5.6-luna", "claude-sonnet-5", ""] {
        assert!(
            !super::is_command_code_model(model),
            "{model} must not look like Command Code"
        );
    }
}

#[test]
fn slims_bloated_claude_dumps_before_cmd() {
    assert!(super::is_command_code_model(
        "meta/muse-spark-1.2-contributor"
    ));
    assert!(!super::is_command_code_model("grok-4.5"));
    let slim = slim_headless_prompt(
        "<system-reminder>\nClaudex routing data (runtime metadata; values only):\n{\"source\":\"claudex-routing-local-hook\",\"selected_workers\":[]}\n</system-reminder>\nclaudex_effort: high\nclaudex_model: meta/muse-spark-1.2-contributor\n<claudex-agent-id>toolu_cc</claudex-agent-id>\nYou are the model inside Claudex on a provider-native ACP backend.\nShared-workspace safety is mandatory: serialize mutations.\nYou are a provider-native ACP worker. Complete the delegated task.\nRead CLAUDE.md and return the first heading.",
    );
    assert_eq!(slim, "Read CLAUDE.md and return the first heading.");
    let reconstructed = slim_headless_prompt(
        "Continue this Claude Code conversation. The role-tagged history follows:\n[{\"role\":\"user\",\"content\":\"Read CLAUDE.md and output only the first heading.\"}]",
    );
    assert!(reconstructed.contains("Read CLAUDE.md"), "{reconstructed}");
    assert!(!reconstructed.contains("Continue this Claude Code conversation"));
    assert!(!slim.contains("selected_workers"));
    let request = acp::PromptRequest::new(
        "session",
        vec![acp::ContentBlock::Text(acp::TextContent::new(
            "<system-reminder>\nctx-agent-history-search dump\n</system-reminder>\nPONGCC09",
        ))],
    );
    let rendered = prompt_text(&request);
    assert!(rendered.contains("PONGCC09"));
    assert!(rendered.contains("Do not greet"));
    assert!(!rendered.contains("ctx-agent-history-search"));
}

#[test]
fn slim_prompt_keeps_task_when_reminder_is_unclosed_and_truncates() {
    let unclosed = slim_headless_prompt(
        "<system-reminder>\n{\"selected_workers\":[{\"agent\":\"claudex-gpt\"}]}\nRead CLAUDE.md and return the heading.",
    );
    assert!(
        unclosed.contains("Read CLAUDE.md"),
        "unclosed reminder must not drop the delegated task: {unclosed}"
    );
    let truncated = slim_headless_prompt(&format!("keep-tail {}", "x".repeat(2_500)));
    assert!(truncated.len() <= 2_000, "{}", truncated.len());
    assert!(truncated.ends_with('x'));
    assert!(!truncated.contains("keep-tail"));
    let empty = prompt_text(&acp::PromptRequest::new(
        "session",
        vec![acp::ContentBlock::Text(acp::TextContent::new("   \n"))],
    ));
    assert!(empty.is_empty(), "{empty}");
}

#[test]
fn slim_prompt_filters_every_instruction_prefix_and_falls_back() {
    let slim = slim_headless_prompt(
        "\n\
claudex-routing dump\n\
selected_workers appear here\n\
ctx-agent-history-search dump\n\
You are the model inside the Claude Code agent harness.\n\
You are a provider-native ACP worker.\n\
Claudex SubAgent routing on ACP.\n\
Claudex provider-native ACP.\n\
Command Code Muse Spark worker.\n\
Ignore Claudex routing tables.\n\
Keep this delegated task.\n",
    );
    assert_eq!(slim, "Keep this delegated task.");
    let fallback = slim_headless_prompt(
        "claudex_effort: high\nYou are the model inside Claudex on ACP.\nShared-workspace safety is mandatory.\n",
    );
    assert!(
        fallback.contains("You are the model inside Claudex")
            || fallback.contains("Shared-workspace safety"),
        "{fallback}"
    );
    assert!(!fallback.contains("claudex_effort"));
}

#[test]
fn collect_text_walks_arrays_objects_and_ignores_other_json() {
    let request = acp::PromptRequest::new(
        "session",
        vec![
            acp::ContentBlock::Text(acp::TextContent::new("alpha")),
            acp::ContentBlock::Text(acp::TextContent::new("")),
        ],
    );
    let wrapped = prompt_text(&request);
    assert!(wrapped.starts_with("alpha"));
    assert!(slim_headless_prompt("   \n\n").is_empty());
}

#[test]
fn coalesces_on_newlines_and_ascii_punctuation() {
    let mut coalescer = ProgressCoalescer::default();
    assert_eq!(
        coalescer.push(ProgressEvent::Message("line\n".to_owned())),
        vec![ProgressEvent::Message("line\n".to_owned())]
    );
    assert_eq!(
        coalescer.push(ProgressEvent::Thought("done!".to_owned())),
        vec![ProgressEvent::Thought("done!".to_owned())]
    );
    assert_eq!(
        coalescer.push(ProgressEvent::Thought("ok?".to_owned())),
        vec![ProgressEvent::Thought("ok?".to_owned())]
    );
    assert_eq!(
        coalescer.push(ProgressEvent::Message("調査完了！".to_owned())),
        vec![ProgressEvent::Message("調査完了！".to_owned())]
    );
    assert_eq!(
        coalescer.push(ProgressEvent::Message("次は？".to_owned())),
        vec![ProgressEvent::Message("次は？".to_owned())]
    );
    assert_eq!(
        remaining_final_message("REPORT", "other live text"),
        Some("REPORT".to_owned())
    );
    assert_eq!(remaining_final_message("hello world", "hello world"), None);
    assert_eq!(remaining_final_message("hello ", "hello"), None);
}

#[test]
fn events_cover_canned_variants_result_shapes_and_previews() {
    assert!(
        rendered_thoughts(&progress_to_updates(&ProgressEvent::Thought(
            "plain thinking".to_owned()
        )))
        .contains("plain thinking")
    );
    assert!(
        rendered_thoughts(&progress_to_updates(&ProgressEvent::Thought(
            "▶ searching AVITA".to_owned()
        )))
        .contains("searching AVITA")
    );
    assert!(
        rendered_messages(&progress_to_updates(&ProgressEvent::Thought(
            "▶ searching AVITA".to_owned()
        )))
        .is_empty()
    );
    for canned in [
        "次: タスク実行",
        "次: ツールまたは回答",
        "次: トークン待ち",
        "次: 別手段または報告",
        "次: 中断",
        "起動: Command Code headless",
        "実行中: web_search。次: 読む",
        "完了: Read。次: 書く",
        "失敗: shell。次: 報告",
    ] {
        assert!(
            progress_to_updates(&ProgressEvent::Status(canned.to_owned())).is_empty(),
            "{canned}"
        );
    }
    let ok = super::events::TurnResult {
        subtype: "success".to_owned(),
        session_id: None,
        stop_reason: Some("end_turn".to_owned()),
        final_text: "   ".to_owned(),
        error: None,
    };
    assert!(!super::events::result_is_error(&ok));
    assert!(super::events::result_message(&ok).is_empty());
    let stop_error = super::events::TurnResult {
        subtype: "success".to_owned(),
        session_id: None,
        stop_reason: Some("error".to_owned()),
        final_text: String::new(),
        error: None,
    };
    assert!(super::events::result_is_error(&stop_error));
    let subtype = super::events::TurnResult {
        subtype: "cancelled".to_owned(),
        session_id: None,
        stop_reason: None,
        final_text: String::new(),
        error: None,
    };
    assert!(super::events::result_message(&subtype).contains("cancelled"));
    let long_query = "x".repeat(90);
    let line = format!(
        r#"{{"type":"event","event":{{"type":"tool_running","toolCallId":"t1","toolName":"web_search","arguments":{{"q":"{long_query}"}}}}}}"#
    );
    assert!(matches!(
        parse_stdout_line(&line),
        ParsedLine::Progress(ProgressEvent::ToolStarted { description, .. })
            if description.as_deref().is_some_and(|text| text.ends_with('…'))
    ));
    assert!(matches!(
        parse_stdout_line(
            r#"{"type":"event","event":{"type":"tool_running","toolName":"read_file","arguments":["not-an-object"]}}"#
        ),
        ParsedLine::Progress(ProgressEvent::ToolStarted {
            description: None,
            ..
        })
    ));
    let ParsedLine::Result(number_error) =
        parse_stdout_line(r#"{"type":"result","subtype":"error","error":12}"#)
    else {
        panic!("expected numeric error");
    };
    assert_eq!(number_error.error.as_deref(), Some("12"));
}

#[tokio::test]
async fn run_turn_emits_started_before_cmd_exits() {
    let root = TempDir::new().expect("slow cmd dir");
    let program = write_executable(
        root.path(),
        "slow",
        "#!/bin/sh\nsleep 0.4\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"finalText\":\"ok\"}'\n",
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let spec = spec_with_program(program);
    let mut turn = std::pin::pin!(run_turn_emitting(&spec, "hello", None, Some(tx), None));
    let first = tokio::select! {
        event = rx.recv() => event.expect("started event"),
        _ = &mut turn => panic!("cmd finished before Started progress"),
    };
    assert!(matches!(
        first,
        ProgressEvent::Started { ref model, .. } if model == DEFAULT_MODEL
    ));
    let outcome = turn.await.expect("slow turn");
    assert_eq!(outcome.result.final_text, "ok");
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

    let crlf_stderr = write_executable(
        root.path(),
        "crlf-stderr",
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"finalText\":\"STDERR_OK\"}'\nprintf 'warn\\r\\nmore\\n' >&2\nexit 0\n",
    );
    let outcome = run_turn(&spec_with_program(crlf_stderr), "hi", None)
        .await
        .expect("stderr crlf is drained with stdout");
    assert_eq!(outcome.result.final_text, "STDERR_OK");

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

    let invalid_utf8 = write_executable(
        root.path(),
        "invalid-utf8",
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t1\",\"toolName\":\"web_fetch\"}}'\nprintf '\\377\\n'\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-utf8\",\"stopReason\":\"end_turn\",\"finalText\":\"AFTER_INVALID_UTF8\"}'\n",
    );
    let outcome = run_turn(&spec_with_program(invalid_utf8), "hi", None)
        .await
        .expect("invalid utf8 must not fail the ACP turn");
    assert_eq!(outcome.result.final_text, "AFTER_INVALID_UTF8");
    assert!(outcome.progress.iter().any(
        |event| matches!(event, ProgressEvent::ToolStarted { name, .. } if name == "web_fetch")
    ));

    let partial_eof = write_executable(
        root.path(),
        "partial-eof",
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-partial\",\"stopReason\":\"end_turn\",\"finalText\":\"RESULT_THEN_PARTIAL\"}'\nprintf '\\343'\nexit 0\n",
    );
    let outcome = run_turn(&spec_with_program(partial_eof), "hi", None)
        .await
        .expect("trailing incomplete utf8 must keep the JSON result");
    assert_eq!(outcome.result.final_text, "RESULT_THEN_PARTIAL");

    let crash_utf8 = write_executable(
        root.path(),
        "crash-utf8",
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t1\",\"toolName\":\"web_search\"}}'\nprintf '\\377'\nexit 1\n",
    );
    let outcome = run_turn(&spec_with_program(crash_utf8), "hi", None)
        .await
        .expect("mid-stream invalid utf8 still yields an outcome");
    assert_eq!(outcome.exit_code, Some(1));
    assert!(outcome.result.error.as_deref().is_some_and(|error| {
        error.contains("without a JSON result") || error.contains("stdout closed")
    }));
    assert!(outcome.progress.iter().any(
        |event| matches!(event, ProgressEvent::ToolStarted { name, .. } if name == "web_search")
    ));

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
    assert_eq!(first_tool_call(&read).kind, acp::ToolKind::Read);

    let edit = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "2".to_owned(),
        name: "WriteFile".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&edit).kind, acp::ToolKind::Edit);

    let shell = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "3".to_owned(),
        name: "Bash".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&shell).kind, acp::ToolKind::Execute);

    let search = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "4".to_owned(),
        name: "WebSearch".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&search).kind, acp::ToolKind::Search);

    let glob = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "5".to_owned(),
        name: "Glob".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&glob).kind, acp::ToolKind::Read);

    let patch = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "6".to_owned(),
        name: "ApplyPatch".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&patch).kind, acp::ToolKind::Edit);

    let exec = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "7".to_owned(),
        name: "ShellExec".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&exec).kind, acp::ToolKind::Execute);

    let other = progress_to_updates(&ProgressEvent::ToolStarted {
        id: "8".to_owned(),
        name: "Notify".to_owned(),
        description: None,
    });
    assert_eq!(first_tool_call(&other).kind, acp::ToolKind::Other);
}

#[tokio::test]
async fn run_turn_abort_kills_sleeping_cmd_quickly() {
    let root = TempDir::new().expect("sleep cmd dir");
    let program = write_executable(root.path(), "cmd", "#!/bin/sh\nexec sleep 30\n");
    let spec = spec_with_program(program);
    let started = std::time::Instant::now();
    let run = tokio::spawn(async move { run_turn(&spec, "hello", None).await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    run.abort();
    let joined = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .expect("aborted cmd should drop within 2s");
    assert!(joined.expect_err("join after abort").is_cancelled());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "abort took {:?}",
        started.elapsed()
    );
}
