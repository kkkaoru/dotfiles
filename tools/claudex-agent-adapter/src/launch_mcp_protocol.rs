use std::io::Write;

use anyhow::Result;
use serde_json::{Value, json};

use super::{PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION, record_tools_call, write_message};

pub(super) fn handle(message: &Value, ndjson: bool, stdout: &mut impl Write) -> Result<()> {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let id = message.get("id").cloned();
    match method {
        "initialize" => write_message(
            stdout,
            ndjson,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}
                }
            }),
        ),
        "notifications/initialized" => Ok(()),
        "tools/list" => write_message(
            stdout,
            ndjson,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": tools()}
            }),
        ),
        "tools/call" => {
            record_tools_call(message);
            write_message(
                stdout,
                ndjson,
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{
                            "type": "text",
                            "text": "Claudex: SubAgent launch handed to Claude Code. End the turn; do not poll TaskOutput."
                        }],
                        "isError": false
                    }
                }),
            )
        }
        "ping" => write_message(stdout, ndjson, json!({"jsonrpc":"2.0","id":id,"result":{}})),
        _ if id.is_some() => write_message(
            stdout,
            ndjson,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("Method not found: {method}")}
            }),
        ),
        _ => Ok(()),
    }
}

pub(in crate::launch_mcp) fn tools() -> Value {
    let schema = json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "description": {"type": "string", "description": "Short 3-5 word description of the task"},
            "prompt": {"type": "string", "description": "The task for the agent to perform"},
            "subagent_type": {"type": "string", "description": "Claudex worker type from selected_workers"},
            "run_in_background": {"type": "boolean", "description": "Prefer true for agents panel tracking"},
            "claudex_model": {"type": "string", "description": "Exact worker model id from selected_workers"},
            "claudex_effort": {"type": "string", "description": "Worker effort from selected_workers"}
        },
        "required": ["description", "prompt"]
    });
    json!([
        {
            "name": "Agent",
            "description": "Launch a Claude Code SubAgent through Claudex. Prefer run_in_background=true and selected_workers subagent_type + claudex_model. After launch, end the turn; do not poll.",
            "inputSchema": schema
        },
        {
            "name": "Task",
            "description": "Launch a Claude Code Task SubAgent through Claudex. Prefer run_in_background=true. After launch, end the turn; do not poll.",
            "inputSchema": schema
        }
    ])
}
