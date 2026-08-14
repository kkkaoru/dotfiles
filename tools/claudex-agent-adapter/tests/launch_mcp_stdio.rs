use std::{
    fs,
    io::Write as _,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

#[test]
fn binary_runs_the_real_launch_mcp_stdio_wrapper() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .arg("mcp-claudex-launch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start launch MCP stdio wrapper");
    child
        .stdin
        .take()
        .expect("launch MCP stdin")
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":21,\"method\":\"ping\"}\n")
        .expect("write launch MCP request");
    let output = child.wait_with_output().expect("wait for launch MCP");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("decode launch MCP response");
    assert_eq!(response["id"], 21);
    assert_eq!(response["result"], serde_json::json!({}));
}

#[test]
fn launch_mcp_child_keeps_protocol_bytes_and_emits_a_redacted_ndjson_timeline() {
    let fixture = tempfile::tempdir().expect("launch MCP trace fixture");
    let queue = fixture.path().join("launch-queue.jsonl");
    let output = run_launch_mcp(&ndjson_transcript(), &queue);

    assert!(output.status.success());
    assert_eq!(output.stdout, expected_ndjson_stdout());
    assert_timeline_is_redacted(&output.stderr);
    assert_handoff_was_recorded(&queue);
}

#[test]
fn launch_mcp_child_keeps_protocol_bytes_and_emits_a_redacted_content_length_timeline() {
    let fixture = tempfile::tempdir().expect("launch MCP trace fixture");
    let queue = fixture.path().join("launch-queue.jsonl");
    let output = run_launch_mcp(&content_length_transcript(), &queue);

    assert!(output.status.success());
    assert_eq!(output.stdout, expected_content_length_stdout());
    assert_timeline_is_redacted(&output.stderr);
    assert_handoff_was_recorded(&queue);
}

fn run_launch_mcp(input: &[u8], queue: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .arg("mcp-claudex-launch")
        .env("CLAUDEX_LOG_FORMAT", "json")
        .env("RUST_LOG", "info")
        .env("CLAUDEX_LAUNCH_QUEUE", queue)
        .env("CLAUDEX_LAUNCH_OWNER", "mcp-secret-owner")
        .env("MCP_SECRET_ENV", "mcp-secret-env")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start launch MCP child");
    child
        .stdin
        .take()
        .expect("launch MCP stdin")
        .write_all(input)
        .expect("write MCP transcript");
    child.wait_with_output().expect("wait for launch MCP child")
}

fn ndjson_transcript() -> Vec<u8> {
    let mut transcript = Vec::new();
    for message in request_messages() {
        serde_json::to_writer(&mut transcript, &message).expect("encode NDJSON request");
        transcript.push(b'\n');
    }
    transcript
}

fn content_length_transcript() -> Vec<u8> {
    let mut transcript = Vec::new();
    for message in request_messages() {
        let body = serde_json::to_vec(&message).expect("encode framed request");
        transcript.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        transcript.extend_from_slice(&body);
    }
    transcript
}

fn request_messages() -> Vec<Value> {
    vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"Task",
                "arguments":{
                    "description":"mcp-secret-description",
                    "prompt":"mcp-secret-prompt",
                    "claudex_model":"mcp-secret-model",
                    "cwd":"/mcp-secret-path"
                }
            }
        }),
    ]
}

fn expected_ndjson_stdout() -> Vec<u8> {
    let mut stdout = Vec::new();
    for response in expected_responses() {
        serde_json::to_writer(&mut stdout, &response).expect("encode expected NDJSON response");
        stdout.push(b'\n');
    }
    stdout
}

fn expected_content_length_stdout() -> Vec<u8> {
    let mut stdout = Vec::new();
    for response in expected_responses() {
        let body = serde_json::to_vec(&response).expect("encode expected framed response");
        stdout.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        stdout.extend_from_slice(&body);
    }
    stdout
}

fn expected_responses() -> Vec<Value> {
    vec![
        json!({
            "jsonrpc":"2.0",
            "id":1,
            "result":{
                "protocolVersion":"2024-11-05",
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"claudex-launch","version":"2.0.0"}
            }
        }),
        json!({"jsonrpc":"2.0","id":2,"result":{"tools":launch_tools()}}),
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "result":{
                "content":[{"type":"text","text":"Claudex: SubAgent launch handed to Claude Code. End the turn; do not poll TaskOutput."}],
                "isError":false
            }
        }),
    ]
}

fn launch_tools() -> Value {
    let schema = json!({
        "type":"object",
        "additionalProperties":true,
        "properties":{
            "description":{"type":"string","description":"Short 3-5 word description of the task"},
            "prompt":{"type":"string","description":"The task for the agent to perform"},
            "subagent_type":{"type":"string","description":"Claudex worker type from selected_workers"},
            "run_in_background":{"type":"boolean","description":"Prefer true for agents panel tracking"},
            "claudex_model":{"type":"string","description":"Exact worker model id from selected_workers"},
            "claudex_effort":{"type":"string","description":"Worker effort from selected_workers"}
        },
        "required":["description","prompt"]
    });
    json!([
        {
            "name":"Agent",
            "description":"Launch a Claude Code SubAgent through Claudex. Prefer run_in_background=true and selected_workers subagent_type + claudex_model. After launch, end the turn; do not poll.",
            "inputSchema":schema
        },
        {
            "name":"Task",
            "description":"Launch a Claude Code Task SubAgent through Claudex. Prefer run_in_background=true. After launch, end the turn; do not poll.",
            "inputSchema":schema
        }
    ])
}

fn assert_timeline_is_redacted(stderr: &[u8]) {
    let stderr = String::from_utf8_lossy(stderr);
    for label in [
        "launch MCP child started",
        "launch MCP request",
        "launch MCP notification received",
        "launch MCP result",
    ] {
        assert!(
            stderr.contains(label),
            "missing {label:?} from MCP timeline"
        );
    }
    for method in [
        "initialize",
        "notifications/initialized",
        "tools/list",
        "tools/call",
    ] {
        assert!(
            stderr.contains(method),
            "missing {method:?} from MCP timeline"
        );
    }
    assert!(stderr.contains(r#""tool_name":"Task""#));
    assert!(stderr.contains(r#""tool_count":2"#));
    assert!(stderr.contains("Agent"));
    assert!(stderr.contains("Task"));
    assert!(!stderr.contains("launch MCP notification handled"));
    for secret in [
        "mcp-secret-description",
        "mcp-secret-prompt",
        "mcp-secret-model",
        "mcp-secret-owner",
        "mcp-secret-env",
        "/mcp-secret-path",
        "arguments",
        "CLAUDEX_LAUNCH_OWNER",
        "MCP_SECRET_ENV",
    ] {
        assert!(!stderr.contains(secret), "MCP trace leaked {secret:?}");
    }
}

fn assert_handoff_was_recorded(queue: &Path) {
    let queue = fs::read_to_string(queue).expect("MCP call queue");
    let records: Vec<Value> = queue
        .lines()
        .map(|line| serde_json::from_str(line).expect("MCP queue record"))
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["name"], "Task");
    assert_eq!(records[0]["method"], "tools/call");
    assert_eq!(records[0]["arguments"]["prompt"], "mcp-secret-prompt");
}
