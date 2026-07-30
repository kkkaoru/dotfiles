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
    if prompt.contains("SUBSCRIPTION_PARENT_SYNTHESIS") {
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"NESTED_CHILD_RESULT_RECEIVED","usage":{"output_tokens":4}
            })
        );
        return;
    }
    if prompt.contains("SUBSCRIPTION_NESTED_AGENT")
        && argument(&arguments, "--output-format") == "stream-json"
    {
        send_subscription_tool(
            "nested-child",
            "child research",
            prompt
                .contains("USE_AGENT_MODEL")
                .then_some("claude-opus-4-8"),
        );
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"nested child launched","usage":{"output_tokens":4}
            })
        );
        return;
    }
    if prompt.contains("SUBSCRIPTION_WEB_TOOL_ROUND")
        && prompt.contains("SYNTHETIC_SEARCH_RESULT")
        && prompt.contains("SYNTHETIC_FETCH_RESULT")
    {
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"SYNTHETIC_WEB_RESULTS_RECEIVED","usage":{"output_tokens":6}
            })
        );
        return;
    }
    if prompt.contains("SERVER_WEB_SEARCH_HANDOFF") || prompt.contains("SYNTHETIC_SEARCH_HANDOFF") {
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"[{\"title\":\"Synthetic result\",\"url\":\"https://example.test/search-result\",\"page_age\":null,\"encrypted_content\":\"synthetic\"}]",
                "usage":{"output_tokens":8}
            })
        );
        return;
    }
    if prompt.contains("SUBSCRIPTION_WEB_TOOL_ROUND")
        && argument(&arguments, "--output-format") == "stream-json"
    {
        send_stream_delta("出典");
        send_subscription_named_tool(
            "web-search-1",
            "WebSearch",
            json!({"query":"SYNTHETIC_COMPANY_OFFICIAL"}),
            Some("outer-agent"),
        );
        send_subscription_named_tool(
            "web-fetch-1",
            "WebFetch",
            json!({"url":"https://example.test/fixture"}),
            Some("outer-agent"),
        );
        println!(
            "{}",
            json!({
                "type":"result","subtype":"success","is_error":false,
                "result":"web tools requested","usage":{"output_tokens":6}
            })
        );
        return;
    }
    if prompt.contains("SUBSCRIPTION_PARALLEL_TOOLS")
        && argument(&arguments, "--output-format") == "stream-json"
    {
        send_subscription_tool("tool-alpha", "alpha", None);
        send_stream_delta("INNER_TOOL_REJECTION_MUST_NOT_LEAK");
        send_subscription_tool("tool-beta", "beta", None);
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

fn send_subscription_tool(id: &str, description: &str, model: Option<&str>) {
    let mut input = json!({
        "description":description,
        "prompt":format!("complete {description}"),
        "subagent_type":"claude"
    });
    if let Some(model) = model {
        input["claudex_model"] = json!(model);
    }
    send_subscription_named_tool(id, "Agent", input, None);
}

fn send_subscription_named_tool(
    id: &str,
    name: &str,
    input: serde_json::Value,
    parent_tool_use_id: Option<&str>,
) {
    println!(
        "{}",
        json!({
            "type":"assistant", "parent_tool_use_id":parent_tool_use_id,
            "message":{
                "usage":{"output_tokens":3},
                "content":[{
                    "type":"tool_use", "id":id, "name":name,
                    "input":input
                }]
            }
        })
    );
    io::stdout().flush().expect("flush subscription tool");
}
