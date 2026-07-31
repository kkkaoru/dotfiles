use std::{
    io::{self, Read, Write},
    thread,
    time::Duration,
};

use serde_json::json;

fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    exit_forced_failure(&arguments);
    let backpressure = argument(&arguments, "--model") == "test-backpressure-model";
    emit_backpressure(&backpressure);
    let prompt = read_prompt();
    if handle_stream_prompt(&arguments, &prompt) {
        return;
    }
    println!(
        "{}",
        json!({
            "type":"result", "subtype":"success",
            "result": result_for(&arguments, &prompt, backpressure)
        })
    );
}

fn exit_forced_failure(arguments: &[String]) {
    if arguments.windows(2).any(|pair| {
        pair.first().map(String::as_str) == Some("--model")
            && pair.get(1).map(String::as_str) == Some("test-failing-model")
    }) {
        eprintln!("forced subscription failure");
        std::process::exit(7);
    }
}

fn emit_backpressure(backpressure: &bool) {
    if !backpressure {
        return;
    }
    // Fill stdout before reading stdin. Sequential parent I/O deadlocks when both
    // payloads exceed the OS pipe capacity; concurrent parent I/O must drain this.
    io::stdout()
        .write_all(&vec![b' '; 128 * 1_024])
        .expect("write subscription backpressure fixture");
    io::stdout()
        .flush()
        .expect("flush subscription backpressure fixture");
}

fn read_prompt() -> String {
    let mut prompt = String::new();
    io::stdin()
        .read_to_string(&mut prompt)
        .expect("read subscription prompt");
    prompt
}

fn handle_stream_prompt(arguments: &[String], prompt: &str) -> bool {
    if argument(arguments, "--output-format") != "stream-json" {
        return false;
    }
    if prompt.contains("SUBSCRIPTION_STREAM_DELAY") {
        send_stream_delta("STREAM_FIRST");
        thread::sleep(Duration::from_millis(200));
        send_stream_delta("STREAM_SECOND");
        print_result("STREAM_FIRSTSTREAM_SECOND", 4);
        return true;
    }
    if prompt.contains("SUBSCRIPTION_PARALLEL_TOOLS") {
        send_subscription_tool("tool-alpha", "alpha");
        send_stream_delta("INNER_TOOL_REJECTION_MUST_NOT_LEAK");
        send_subscription_tool("tool-beta", "beta");
        print_result("inner tools were bridged", 9);
        return true;
    }
    if prompt.contains("SUBSCRIPTION_FOLLOW_UP_LAUNCH") {
        send_routed_subscription_tool();
        print_result("launch emitted", 4);
        return true;
    }
    if prompt.contains("SUBSCRIPTION_FOLLOW_UP_NO_LAUNCH") {
        send_stream_delta("SUBSCRIPTION_DIRECT_RESULT");
        print_result("SUBSCRIPTION_DIRECT_RESULT", 4);
        return true;
    }
    false
}

fn print_result(result: &str, output_tokens: u64) {
    println!(
        "{}",
        json!({
            "type":"result", "subtype":"success", "is_error":false,
            "result":result, "usage":{"output_tokens":output_tokens}
        })
    );
}

fn result_for(arguments: &[String], prompt: &str, backpressure: bool) -> String {
    if backpressure {
        format!("BACKPRESSURE_OK:{}", prompt.len())
    } else if prompt.contains("SUBSCRIPTION_EMPTY") {
        String::new()
    } else if prompt.contains("SUBSCRIPTION_ROUTE") {
        format!(
            "{}|{}|{}|{}|{}",
            argument(arguments, "--model"),
            argument(arguments, "--effort"),
            argument(arguments, "--tools"),
            argument(arguments, "--allowedTools"),
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "missing-cwd".to_owned())
        )
    } else if prompt.contains("rigorous advisor") && prompt.contains("CURRENT_TURN_ADVISOR") {
        "MOCK_ADVISOR_CURRENT_TURN".to_owned()
    } else if prompt.contains("rigorous advisor") {
        "MOCK_ADVISOR_RESULT".to_owned()
    } else {
        "MOCK_COLLABORATOR_RESULT".to_owned()
    }
}

fn argument<'a>(arguments: &'a [String], name: &str) -> &'a str {
    arguments
        .windows(2)
        .find(|pair| pair.first().map(String::as_str) == Some(name))
        .and_then(|pair| pair.get(1))
        .map_or("missing", String::as_str)
}

fn send_stream_delta(text: &str) {
    println!(
        "{}",
        json!({
            "type":"stream_event",
            "event":{"type":"content_block_delta","delta":{"type":"text_delta","text":text}}
        })
    );
    io::stdout().flush().expect("flush stream delta");
}

fn send_subscription_tool(id: &str, description: &str) {
    println!(
        "{}",
        json!({
            "type":"assistant", "parent_tool_use_id":null,
            "message":{
                "usage":{"output_tokens":3},
                "content":[{
                    "type":"tool_use", "id":id, "name":"Agent",
                    "input":{
                        "description":description,
                        "prompt":format!("complete {description}"),
                        "subagent_type":"claudex-gpt-spark"
                    }
                }]
            }
        })
    );
    io::stdout().flush().expect("flush subscription tool");
}

fn send_routed_subscription_tool() {
    println!(
        "{}",
        json!({
            "type":"assistant", "parent_tool_use_id":null,
            "message":{
                "usage":{"output_tokens":3},
                "content":[{
                    "type":"tool_use", "id":"subscription-follow-up", "name":"Agent",
                    "input":{
                        "description":"subscription follow-up",
                        "prompt":"complete the independent follow-up",
                        "subagent_type":"general-purpose",
                        "claudex_model":"test-main-model"
                    }
                }]
            }
        })
    );
    io::stdout()
        .flush()
        .expect("flush routed subscription tool");
}
