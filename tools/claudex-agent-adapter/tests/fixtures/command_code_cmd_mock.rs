use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
    thread,
    time::Duration,
};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Ok(path) = env::var("COMMAND_CODE_CMD_MOCK_TRACE") {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open command-code mock trace");
        writeln!(file, "{}", serde_json::json!({ "args": args })).expect("write mock trace");
    }
    let prompt = args.last().cloned().unwrap_or_default();
    if prompt.contains("STREAM_DELAY") {
        stream_delay();
        return;
    }
    if prompt.contains("WEB_SEARCH_AVITA") {
        web_search_avita();
        return;
    }
    if prompt.contains("CANNED_STATUS") {
        canned_then_answer();
        return;
    }
    match env::var("COMMAND_CODE_CMD_MOCK_MODE")
        .unwrap_or_default()
        .as_str()
    {
        "auth-fail" => std::process::exit(3),
        "max-turns" => {
            emit_line(
                r#"{"type":"result","subtype":"max_turns","sessionId":"cc-max","stopReason":"max_turns","finalText":"partial"}"#,
            );
            std::process::exit(8);
        }
        "credits" => std::process::exit(10),
        "boom" => std::process::exit(1),
        "slow" => {
            emit_line(
                r#"{"type":"event","event":{"type":"tool_running","toolCallId":"slow-1","toolName":"read_file","description":"waiting"}}"#,
            );
            thread::sleep(Duration::from_secs(30));
            emit_line(
                r#"{"type":"result","subtype":"success","sessionId":"cc-slow","stopReason":"end_turn","finalText":"TOO_LATE"}"#,
            );
        }
        "wait-release" => wait_for_release_then_succeed(),
        "plain-text" => {
            println!("not-json progress");
            emit_line(
                r#"{"type":"result","subtype":"success","sessionId":"cc-text","stopReason":"end_turn","finalText":"PLAIN_OK"}"#,
            );
        }
        "empty-event" => {
            println!();
            emit_line(r#"{"type":"event","event":{}}"#);
            emit_line(
                r#"{"type":"result","subtype":"success","sessionId":"cc-empty","finalText":"EMPTY_EVENT_OK"}"#,
            );
        }
        _ => default_success(&args),
    }
}

fn stream_delay() {
    emit_line(
        r#"{"type":"event","event":{"type":"tool_running","toolCallId":"t-live","toolName":"read_file","description":"README.md"}}"#,
    );
    emit_line(
        r#"{"type":"event","event":{"type":"text_delta","delta":"LIVE_DELTA_BEFORE_SLEEP"}}"#,
    );
    let _ = io::stdout().flush();
    thread::sleep(Duration::from_millis(200));
    emit_line(r#"{"type":"event","event":{"type":"text_delta","delta":" MORE_AFTER_SLEEP"}}"#);
    emit_line(
        r#"{"type":"result","subtype":"success","sessionId":"cc-stream","stopReason":"end_turn","finalText":"STREAM_DELAY_OK"}"#,
    );
}

fn web_search_avita() {
    emit_line(
        r#"{"type":"event","event":{"type":"tool_running","toolCallId":"t-search","toolName":"web_search","query":"AVITA株式会社"}}"#,
    );
    emit_line(
        r#"{"type":"event","event":{"type":"tool_completed","toolCallId":"t-search","toolName":"web_search"}}"#,
    );
    emit_line(
        r#"{"type":"result","subtype":"success","sessionId":"cc-search","stopReason":"end_turn","finalText":"WEB_SEARCH_OK"}"#,
    );
}

fn canned_then_answer() {
    emit_line(
        r#"{"type":"event","event":{"type":"text_delta","delta":"● 実行中: web_search。次: ツール結果待ち"}}"#,
    );
    emit_line(r#"{"type":"event","event":{"type":"text_delta","delta":"AVITA findings"}}"#);
    emit_line(
        r#"{"type":"result","subtype":"success","sessionId":"cc-canned","stopReason":"end_turn","finalText":"CANNED_STATUS_OK"}"#,
    );
}

fn wait_for_release_then_succeed() {
    emit_line(
        r#"{"type":"event","event":{"type":"tool_running","toolCallId":"t-wait","toolName":"read_file","description":"waiting"}}"#,
    );
    let _ = io::stdout().flush();
    let release = env::var("COMMAND_CODE_CMD_MOCK_RELEASE").unwrap_or_default();
    let started = std::time::Instant::now();
    while !release.is_empty() && !PathBuf::from(&release).exists() {
        if started.elapsed() > Duration::from_secs(30) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    emit_line(
        r#"{"type":"result","subtype":"success","sessionId":"cc-wait","stopReason":"end_turn","finalText":"WAIT_RELEASE_OK"}"#,
    );
}

fn default_success(args: &[String]) {
    let prompt = args.last().cloned().unwrap_or_default();
    let resume = args
        .windows(2)
        .find(|window| window[0] == "--resume")
        .map(|window| window[1].as_str())
        .unwrap_or("");
    emit_line(
        r#"{"type":"event","event":{"type":"tool_running","toolCallId":"t1","toolName":"read_file","description":"README.md"}}"#,
    );
    emit_line(
        r#"{"type":"event","event":{"type":"tool_completed","toolCallId":"t1","toolName":"read_file"}}"#,
    );
    emit_line(
        r#"{"type":"event","event":{"type":"tool_failed","toolCallId":"t2","toolName":"shell","error":"denied"}}"#,
    );
    let session = if resume.is_empty() {
        "cc-session-1"
    } else {
        "cc-session-resume"
    };
    let text = if prompt.contains("COMMAND_CODE_HEADLESS_OK") || prompt.is_empty() {
        "COMMAND_CODE_HEADLESS_OK".to_owned()
    } else {
        format!("COMMAND_CODE_HEADLESS_OK:{prompt}:{resume}")
    };
    let payload = serde_json::json!({
        "type": "result",
        "subtype": "success",
        "sessionId": session,
        "stopReason": "end_turn",
        "finalText": text,
        "usage": {},
        "durationMs": 12
    });
    emit_line(&payload.to_string());
    let _ = io::stdout().flush();
    let _ = PathBuf::from(".");
}

fn emit_line(line: &str) {
    println!("{line}");
}
