use std::{
    io::{self, Read, Write},
    thread,
    time::Duration,
};

use serde_json::json;

fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments.windows(2).any(|pair| {
        pair.first().map(String::as_str) == Some("--model")
            && pair.get(1).map(String::as_str) == Some("test-failing-model")
    }) {
        eprintln!("forced subscription failure");
        std::process::exit(7);
    }
    let backpressure = argument(&arguments, "--model") == "test-backpressure-model";
    if backpressure {
        // Fill stdout before reading stdin. Sequential parent I/O deadlocks when both
        // payloads exceed the OS pipe capacity; concurrent parent I/O must drain this.
        io::stdout()
            .write_all(&vec![b' '; 128 * 1_024])
            .expect("write subscription backpressure fixture");
        io::stdout()
            .flush()
            .expect("flush subscription backpressure fixture");
    }
    let mut prompt = String::new();
    io::stdin()
        .read_to_string(&mut prompt)
        .expect("read subscription prompt");
    if prompt.contains("SUBSCRIPTION_STREAM_DELAY")
        && argument(&arguments, "--output-format") == "stream-json"
    {
        send_stream_delta("STREAM_FIRST");
        thread::sleep(Duration::from_millis(200));
        send_stream_delta("STREAM_SECOND");
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"STREAM_FIRSTSTREAM_SECOND","usage":{"output_tokens":4}
            })
        );
        return;
    }
    if prompt.contains("SUBSCRIPTION_PARALLEL_TOOLS")
        && argument(&arguments, "--output-format") == "stream-json"
    {
        send_subscription_tool("tool-alpha", "alpha");
        send_stream_delta("INNER_TOOL_REJECTION_MUST_NOT_LEAK");
        send_subscription_tool("tool-beta", "beta");
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"inner tools were bridged","usage":{"output_tokens":9}
            })
        );
        return;
    }
    let result = if backpressure {
        format!("BACKPRESSURE_OK:{}", prompt.len())
    } else if prompt.contains("SUBSCRIPTION_EMPTY") {
        String::new()
    } else if prompt.contains("SUBSCRIPTION_ROUTE") {
        format!(
            "{}|{}|{}|{}|{}",
            argument(&arguments, "--model"),
            argument(&arguments, "--effort"),
            argument(&arguments, "--tools"),
            argument(&arguments, "--allowedTools"),
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
    };
    println!(
        "{}",
        json!({"type":"result","subtype":"success","result":result})
    );
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
                        "subagent_type":"claudex-gpt"
                    }
                }]
            }
        })
    );
    io::stdout().flush().expect("flush subscription tool");
}
