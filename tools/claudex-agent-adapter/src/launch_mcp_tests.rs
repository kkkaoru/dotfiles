use std::{fs, io::Cursor, sync::Mutex};

use serde_json::{Value, json};

use super::{
    handle, launch_owner_from_params, launch_queue_path, read_message, record_tools_call_to,
    run_with_io, sanitize_launch_owner, tools, write_message,
};

static LAUNCH_OWNER_ENV_LOCK: Mutex<()> = Mutex::new(());

fn ndjson_response(message: Value) -> Value {
    let mut output = Vec::new();
    handle(&message, true, &mut output).expect("handle MCP message");
    serde_json::from_slice(&output).expect("NDJSON response")
}

#[test]
fn handles_every_protocol_method_and_notification_shape() {
    let initialized = ndjson_response(json!({
        "jsonrpc":"2.0","id":1,"method":"initialize","params":{}
    }));
    assert_eq!(initialized["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "claudex-launch"
    );

    let listed = ndjson_response(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert_eq!(listed["result"]["tools"], tools());
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 2);

    let called = ndjson_response(json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"Task","arguments":{"description":"d","prompt":"p"}}
    }));
    assert_eq!(called["result"]["isError"], false);
    assert!(
        called["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("SubAgent launch handed")
    );

    assert_eq!(
        ndjson_response(json!({"jsonrpc":"2.0","id":4,"method":"ping"}))["result"],
        json!({})
    );
    let unknown = ndjson_response(json!({"jsonrpc":"2.0","id":5,"method":"unknown"}));
    assert_eq!(unknown["error"]["code"], -32601);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown")
    );

    for notification in [
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","method":"unknown"}),
        json!({}),
    ] {
        let mut output = Vec::new();
        handle(&notification, true, &mut output).expect("handle notification");
        assert!(output.is_empty());
    }
}

#[test]
fn reads_ndjson_and_content_length_frames_with_blank_prefixes() {
    let mut ndjson = Cursor::new(b"\n  \n{\"id\":1}\n".as_slice());
    let (message, mode) = read_message(&mut ndjson).unwrap().expect("NDJSON message");
    assert_eq!(message["id"], 1);
    assert!(mode);
    assert!(read_message(&mut ndjson).unwrap().is_none());

    let body = br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
    let framed = format!(
        "X-Ignored: yes\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    let (message, mode) = read_message(&mut Cursor::new(framed)).unwrap().unwrap();
    assert_eq!(message["method"], "ping");
    assert!(!mode);

    for incomplete in [
        "Content-Length: invalid\r\n\r\n",
        "X-Ignored: yes\r\n\r\n",
        "Content-Length: 2\r\n",
    ] {
        assert!(
            read_message(&mut Cursor::new(incomplete))
                .unwrap()
                .is_none()
        );
    }
    assert!(read_message(&mut Cursor::new("{bad}\n")).is_err());

    let ping = br#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#;
    let zero_then_ping = format!(
        "Content-Length: 0\r\n\r\nContent-Length: {}\r\n\r\n{}",
        ping.len(),
        String::from_utf8_lossy(ping)
    );
    let (message, mode) = read_message(&mut Cursor::new(zero_then_ping))
        .unwrap()
        .expect("empty Content-Length must not stop the MCP server");
    assert_eq!(message["id"], 3);
    assert!(!mode);

    let header_without_colon = format!(
        "NotAHeader\r\nContent-Length: {}\r\n\r\n{}",
        ping.len(),
        String::from_utf8_lossy(ping)
    );
    let (message, mode) = read_message(&mut Cursor::new(header_without_colon))
        .unwrap()
        .expect("header without colon must be ignored");
    assert_eq!(message["id"], 3);
    assert!(!mode);
}

#[test]
fn run_with_io_switches_to_ndjson_then_stops_on_eof() {
    let ping = br#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#;
    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}}\nContent-Length: {}\r\n\r\n{}",
        ping.len(),
        String::from_utf8_lossy(ping)
    );
    let mut stdout = Vec::new();
    run_with_io(&mut Cursor::new(input), &mut stdout).expect("stdio loop");
    let text = String::from_utf8(stdout).expect("utf8");
    assert!(text.contains("\"id\":7"));
    assert!(text.contains("\"id\":8"));
    run_with_io(&mut Cursor::new(""), &mut Vec::new()).expect("empty stdin");
}

#[test]
fn sanitizes_launch_owner_and_queue_paths() {
    assert_eq!(sanitize_launch_owner("@@@"), "___");
    assert_eq!(sanitize_launch_owner(""), "unknown");
    assert_eq!(sanitize_launch_owner("session-a_1.0"), "session-a_1.0");
    let long = "a".repeat(200);
    let sanitized = sanitize_launch_owner(&long);
    assert_eq!(sanitized.len(), 128);
    assert!(sanitized.chars().all(|c| c == 'a'));

    let cache = std::path::Path::new("/tmp/claudex-cache");
    assert_eq!(
        launch_queue_path(cache, None),
        cache.join("launch-queue.jsonl")
    );
    assert_eq!(
        launch_queue_path(cache, Some("  session a  ")),
        cache.join("launch-queue.session_a.jsonl")
    );
    assert_eq!(
        launch_owner_from_params(&json!({"claudexLaunchOwner":"  "})),
        None
    );
    assert_eq!(
        launch_owner_from_params(&json!({"claudexLaunchOwner":"session-a"})),
        Some("session-a".to_owned())
    );
}

#[test]
fn reads_array_ndjson_and_lf_only_content_length_frames() {
    let (message, mode) = read_message(&mut Cursor::new(b"[{\"id\":9}]\n".as_slice()))
        .unwrap()
        .expect("array NDJSON");
    assert_eq!(message[0]["id"], 9);
    assert!(mode);

    let body = br#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#;
    let framed = format!(
        "Content-Length: {}\n\n{}",
        body.len(),
        String::from_utf8_lossy(body)
    );
    let (message, mode) = read_message(&mut Cursor::new(framed)).unwrap().unwrap();
    assert_eq!(message["id"], 4);
    assert!(!mode);
}

#[test]
fn writes_both_transport_encodings() {
    let message = json!({"jsonrpc":"2.0","id":7,"result":{}});
    let mut ndjson = Vec::new();
    write_message(&mut ndjson, true, message.clone()).unwrap();
    assert!(ndjson.ends_with(b"\n"));
    assert_eq!(serde_json::from_slice::<Value>(&ndjson).unwrap(), message);

    let mut framed = Vec::new();
    write_message(&mut framed, false, message).unwrap();
    let framed = String::from_utf8(framed).unwrap();
    assert!(framed.starts_with("Content-Length: "));
    assert!(framed.contains("\r\n\r\n{\"id\":7,\"jsonrpc\":\"2.0\",\"result\":{}}"));
}

#[test]
fn records_calls_to_each_configured_queue_with_defaults() {
    let _guard = LAUNCH_OWNER_ENV_LOCK.lock().expect("launch owner env lock");
    let previous = std::env::var_os("CLAUDEX_LAUNCH_OWNER");
    unsafe { std::env::remove_var("CLAUDEX_LAUNCH_OWNER") };
    let root = tempfile::tempdir().expect("MCP log fixture");
    let queue = root.path().join("nested/queue.jsonl");
    let log = root.path().join("launch.log");
    record_tools_call_to(
        &json!({"method":"tools/call"}),
        123.5,
        [queue.clone(), log.clone()],
    );

    for path in [queue, log] {
        let payload: Value =
            serde_json::from_str(fs::read_to_string(path).unwrap().trim()).unwrap();
        assert_eq!(payload["ts"], 123.5);
        assert_eq!(payload["name"], "Agent");
        assert_eq!(payload["arguments"], json!({}));
        assert_eq!(payload["method"], "tools/call");
        assert_eq!(payload["params"], Value::Null);
        assert!(payload.get("owner").is_none());
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("CLAUDEX_LAUNCH_OWNER", value) },
        None => unsafe { std::env::remove_var("CLAUDEX_LAUNCH_OWNER") },
    }
}

#[test]
fn records_launch_owner_when_env_is_set() {
    let _guard = LAUNCH_OWNER_ENV_LOCK.lock().expect("launch owner env lock");
    let previous = std::env::var_os("CLAUDEX_LAUNCH_OWNER");
    unsafe { std::env::set_var("CLAUDEX_LAUNCH_OWNER", "session-a") };
    let root = tempfile::tempdir().expect("MCP owner fixture");
    let queue = root.path().join("queue.jsonl");
    record_tools_call_to(
        &json!({"method":"tools/call","params":{"name":"Agent","arguments":{"prompt":"p"}}}),
        1.0,
        [queue.clone()],
    );
    let payload: Value = serde_json::from_str(fs::read_to_string(queue).unwrap().trim()).unwrap();
    assert_eq!(payload["owner"], "session-a");
    match previous {
        Some(value) => unsafe { std::env::set_var("CLAUDEX_LAUNCH_OWNER", value) },
        None => unsafe { std::env::remove_var("CLAUDEX_LAUNCH_OWNER") },
    }
}

#[test]
fn record_tools_call_tolerates_root_and_unwritable_targets() {
    let root = tempfile::tempdir().expect("unwritable MCP fixture");
    let blocker = root.path().join("not-a-directory");
    fs::write(&blocker, "x").expect("file blocker");
    let nested = blocker.join("queue.jsonl");
    record_tools_call_to(
        &json!({"method":"tools/call"}),
        1.0,
        [std::path::PathBuf::from("/"), nested.clone()],
    );
    assert!(
        !nested.exists(),
        "parent-as-file targets must not create a queue file"
    );
}

#[test]
fn run_with_io_drains_ndjson_until_eof() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        "\n",
    );
    let mut reader = Cursor::new(input.as_bytes());
    let mut stdout = Vec::new();
    run_with_io(&mut reader, &mut stdout).expect("stdio MCP loop");
    let text = String::from_utf8(stdout).expect("utf8");
    assert!(text.contains("claudex-launch"));
    assert!(text.contains("\"id\":2"));
}
