use std::io::{self, BufRead as _, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut next_thread = 0_u64;
    for line in stdin.lock().lines() {
        let message: Value =
            serde_json::from_str(&line.expect("read request")).expect("JSON request");
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => send(&mut stdout, json!({"id":message["id"],"result":{}})),
            Some("thread/start") => {
                next_thread += 1;
                send(
                    &mut stdout,
                    json!({"id":message["id"],"result":{"thread":{"id":format!("codex-{next_thread}")}}}),
                );
            }
            Some("turn/start") => send_turn(&mut stdout, &message),
            _ => {}
        }
    }
}

fn send_turn(output: &mut impl Write, message: &Value) {
    let thread_id = message
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .unwrap();
    send(
        output,
        json!({"id":message["id"],"result":{"turn":{"id":"turn"}}}),
    );
    send(
        output,
        json!({"method":"item/agentMessage/delta","params":{"threadId":thread_id,"delta":"CODEX_ROUTED_OK"}}),
    );
    send(
        output,
        json!({"method":"turn/completed","params":{"threadId":thread_id,"turn":{"status":"completed"}}}),
    );
}

fn send(output: &mut impl Write, value: Value) {
    writeln!(output, "{value}").expect("write response");
    output.flush().expect("flush response");
}
