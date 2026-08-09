//! Stdio MCP server that exposes Claude Code Agent/Task launch tools.
//!
//! Cursor ACP attaches this as `claudex-launch`. Tool calls are acknowledged
//! here and also appended to a local queue so the adapter can bridge empty ACP
//! `providerTool` cards into Claude Code `tool_use`.

use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "claudex-launch";
const SERVER_VERSION: &str = "2.0.0";
const LAUNCH_QUEUE_FILE: &str = "launch-queue.jsonl";
const MAX_OWNER_FILE_CHARS: usize = 128;

pub(crate) fn sanitize_launch_owner(owner: &str) -> String {
    let mut sanitized = String::new();
    for character in owner.chars() {
        if sanitized.len() >= MAX_OWNER_FILE_CHARS {
            break;
        }
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

pub(crate) fn launch_queue_path(cache: &Path, owner: Option<&str>) -> PathBuf {
    match owner.map(str::trim).filter(|owner| !owner.is_empty()) {
        Some(owner) => cache.join(format!(
            "launch-queue.{}.jsonl",
            sanitize_launch_owner(owner)
        )),
        None => cache.join(LAUNCH_QUEUE_FILE),
    }
}

pub(crate) fn launch_owner_from_params(params: &Value) -> Option<String> {
    params
        .get("claudexLaunchOwner")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|owner| !owner.is_empty())
        .map(str::to_owned)
}

pub fn run_stdio() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut ndjson = false;
    loop {
        let Some((message, mode)) = read_message(&mut reader)? else {
            break;
        };
        if mode {
            ndjson = true;
        }
        handle(&message, ndjson, &mut stdout)?;
    }
    Ok(())
}

fn handle(message: &Value, ndjson: bool, stdout: &mut impl Write) -> Result<()> {
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

fn tools() -> Value {
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

fn record_tools_call(message: &Value) {
    let paths = ["CLAUDEX_LAUNCH_QUEUE", "CLAUDEX_LAUNCH_MCP_LOG"]
        .into_iter()
        .filter_map(env::var_os)
        .map(PathBuf::from);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    record_tools_call_to(message, timestamp, paths);
}

fn record_tools_call_to(message: &Value, timestamp: f64, paths: impl IntoIterator<Item = PathBuf>) {
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Agent");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let owner = env::var("CLAUDEX_LAUNCH_OWNER")
        .ok()
        .map(|owner| owner.trim().to_owned())
        .filter(|owner| !owner.is_empty());
    let mut payload = json!({
        "ts": timestamp,
        "name": name,
        "arguments": arguments,
        "method": message.get("method"),
        "params": params
    });
    if let Some(owner) = owner
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("owner".to_owned(), json!(owner));
    }
    for path in paths {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path)
            && let Ok(line) = serde_json::to_string(&payload)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn write_message(stdout: &mut impl Write, ndjson: bool, message: Value) -> Result<()> {
    let body = serde_json::to_vec(&message).context("serialize MCP message")?;
    if ndjson {
        stdout.write_all(&body)?;
        stdout.write_all(b"\n")?;
    } else {
        write!(stdout, "Content-Length: {}\r\n\r\n", body.len())?;
        stdout.write_all(&body)?;
    }
    stdout.flush()?;
    Ok(())
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<(Value, bool)>> {
    let mut first = String::new();
    if reader.read_line(&mut first)? == 0 {
        return Ok(None);
    }
    let stripped = first.trim();
    if stripped.is_empty() {
        return read_message(reader);
    }
    if stripped.starts_with('{') || stripped.starts_with('[') {
        return Ok(Some((serde_json::from_str(stripped)?, true)));
    }
    let mut headers = std::collections::HashMap::new();
    let mut line = first;
    loop {
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
    }
    let Some(length) = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|length| *length > 0)
    else {
        return read_message(reader);
    };
    let mut body = vec![0_u8; length];
    io::Read::read_exact(reader, &mut body)?;
    Ok(Some((serde_json::from_slice(&body)?, false)))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "launch_mcp_tests.rs"]
mod tests;
