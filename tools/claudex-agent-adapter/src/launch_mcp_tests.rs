use std::{fs, io::Cursor};

use serde_json::{Value, json};

use super::{handle, read_message, record_tools_call_to, tools, write_message};

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
}

#[test]
fn writes_both_transport_encodings() {
    let message = json!({"jsonrpc":"2.0","id":7,"result":{}});
    let mut ndjson = Vec::new();
    write_message(&mut ndjson, true, message.clone()).unwrap();
    assert!(ndjson.ends_with(b"\n"));
    assert_eq!(
        serde_json::from_slice::<Value>(&ndjson).unwrap(),
        message.clone()
    );

    let mut framed = Vec::new();
    write_message(&mut framed, false, message).unwrap();
    let framed = String::from_utf8(framed).unwrap();
    assert!(framed.starts_with("Content-Length: "));
    assert!(framed.contains("\r\n\r\n{\"id\":7,\"jsonrpc\":\"2.0\",\"result\":{}}"));
}

#[test]
fn records_calls_to_each_configured_queue_with_defaults() {
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
    }
}
