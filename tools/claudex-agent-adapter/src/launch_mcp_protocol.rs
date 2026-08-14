use std::io::Write;

use anyhow::Result;
use serde_json::{Value, json};

use super::{PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION, record_tools_call, write_message};

const LAUNCH_TOOL_NAMES: [&str; 2] = ["Agent", "Task"];

pub(super) fn handle(message: &Value, ndjson: bool, stdout: &mut impl Write) -> Result<()> {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let id = message.get("id").cloned();
    match method {
        "initialize" => handle_initialize(id, ndjson, stdout),
        "notifications/initialized" => {
            tracing::info!(mcp_method = method, "launch MCP notification received");
            Ok(())
        }
        "tools/list" => handle_tools_list(id, ndjson, stdout),
        "tools/call" => handle_tools_call(message, id, ndjson, stdout),
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

fn handle_initialize(id: Option<Value>, ndjson: bool, stdout: &mut impl Write) -> Result<()> {
    tracing::info!(
        mcp_method = "initialize",
        status = "received",
        "launch MCP request"
    );
    let result = write_message(
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
    );
    log_response("initialize", &result);
    result
}

fn handle_tools_list(id: Option<Value>, ndjson: bool, stdout: &mut impl Write) -> Result<()> {
    tracing::info!(
        mcp_method = "tools/list",
        status = "received",
        "launch MCP request"
    );
    let result = write_message(
        stdout,
        ndjson,
        json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}),
    );
    log_tools_list_response(&result);
    result
}

fn handle_tools_call(
    message: &Value,
    id: Option<Value>,
    ndjson: bool,
    stdout: &mut impl Write,
) -> Result<()> {
    let tool_name = launch_tool_name(message);
    tracing::info!(
        mcp_method = "tools/call",
        tool_name,
        status = "received",
        "launch MCP request"
    );
    record_tools_call(message);
    let result = write_message(
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
    );
    log_tools_call_response(tool_name, &result);
    result
}

fn launch_tool_name(message: &Value) -> &'static str {
    match message.get("params") {
        None => "missing",
        Some(Value::Object(object)) if !object.contains_key("name") => "missing",
        Some(Value::Object(object)) => match object.get("name") {
            Some(Value::String(name)) => match name.as_str() {
                "Agent" => "Agent",
                "Task" => "Task",
                _ => "other",
            },
            Some(_) => "malformed",
            None => unreachable!("name presence was checked above"),
        },
        Some(_) => "malformed",
    }
}

fn log_response(method: &str, result: &Result<()>) {
    match result {
        Ok(()) => tracing::info!(mcp_method = method, status = "ok", "launch MCP result"),
        Err(_) => tracing::warn!(
            mcp_method = method,
            status = "write_error",
            "launch MCP result"
        ),
    }
}

fn log_tools_call_response(tool_name: &str, result: &Result<()>) {
    match result {
        Ok(()) => {
            tracing::info!(
                mcp_method = "tools/call",
                tool_name,
                status = "ok",
                "launch MCP result"
            )
        }
        Err(_) => tracing::warn!(
            mcp_method = "tools/call",
            tool_name,
            status = "write_error",
            "launch MCP result"
        ),
    }
}

fn log_tools_list_response(result: &Result<()>) {
    match result {
        Ok(()) => tracing::info!(
            mcp_method = "tools/list",
            status = "ok",
            tool_count = LAUNCH_TOOL_NAMES.len(),
            tool_names = ?LAUNCH_TOOL_NAMES,
            "launch MCP result"
        ),
        Err(_) => tracing::warn!(
            mcp_method = "tools/list",
            status = "write_error",
            tool_count = LAUNCH_TOOL_NAMES.len(),
            tool_names = ?LAUNCH_TOOL_NAMES,
            "launch MCP result"
        ),
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
    Value::Array(
        LAUNCH_TOOL_NAMES
            .into_iter()
            .map(|name| {
                let description = match name {
                    "Agent" => "Launch a Claude Code SubAgent through Claudex. Prefer run_in_background=true and selected_workers subagent_type + claudex_model. After launch, end the turn; do not poll.",
                    "Task" => "Launch a Claude Code Task SubAgent through Claudex. Prefer run_in_background=true. After launch, end the turn; do not poll.",
                    _ => unreachable!("launch tool names are fixed"),
                };
                json!({"name": name, "description": description, "inputSchema": schema.clone()})
            })
            .collect(),
    )
}
