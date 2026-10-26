//! Stdio MCP server that shows pi's tool schemas to a subscription `claude -p` child.
//!
//! Claude Code only knows its own built-in tool names, so pi's tools are listed here as
//! `mcp__pi__<name>`. The adapter forwards the model's tool_use to pi and stops the child, so
//! `tools/call` is never answered: pi, not this stub, executes every tool.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const PI_TOOLS_MCP_FLAG: &str = "__pi-tools-mcp";
pub(crate) const PI_MCP_PREFIX: &str = "mcp__pi__";
const PI_TOOLS_ENV: &str = "CLAUDEX_PI_TOOLS";

/// `--mcp-config` JSON that launches this binary as the `pi` server with pi's tool list.
pub(crate) fn pi_mcp_config(program: &std::path::Path, tools: &[Value]) -> String {
    let listed = tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.get("name"),
                "description": tool.get("description").cloned().unwrap_or(json!("")),
                "inputSchema": tool.get("input_schema").cloned()
                    .unwrap_or(json!({"type":"object"})),
            })
        })
        .collect::<Vec<_>>();
    json!({"mcpServers":{"pi":{
        "type":"stdio",
        "command": program,
        "args":[PI_TOOLS_MCP_FLAG],
        "env":{PI_TOOLS_ENV: Value::Array(listed).to_string()},
    }}})
    .to_string()
}

pub(crate) fn pi_allowed_tools(tools: &[Value]) -> String {
    tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(|name| format!("{PI_MCP_PREFIX}{name}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The JSON-RPC reply to one client message; `None` for notifications and `tools/call`.
pub(crate) fn reply(message: &Value, tools: &Value) -> Option<Value> {
    let id = message.get("id")?;
    let result = match message.get("method").and_then(Value::as_str)? {
        "initialize" => json!({
            "protocolVersion": message.pointer("/params/protocolVersion")
                .cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pi", "version": env!("CARGO_PKG_VERSION")},
        }),
        "tools/list" => json!({"tools": tools}),
        "tools/call" => return None,
        "ping" => json!({}),
        _ => {
            return Some(json!({"jsonrpc":"2.0","id":id,
                "error":{"code":-32601,"message":"method not found"}}));
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub async fn serve_stdio() -> Result<()> {
    let tools: Value = serde_json::from_str(
        &std::env::var(PI_TOOLS_ENV).with_context(|| format!("{PI_TOOLS_ENV} is not set"))?,
    )
    .context("parse pi tool list")?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(response) = reply(&message, &tools) {
            stdout.write_all(format!("{response}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn lists_pi_tools_and_never_answers_calls() {
        let tools = json!([{"name":"read","description":"Read","inputSchema":{"type":"object"}}]);
        let init = reply(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
            &tools,
        )
        .expect("initialize reply");
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let list = reply(&json!({"id":2,"method":"tools/list"}), &tools).expect("list reply");
        assert_eq!(list["result"]["tools"], tools);
        assert!(reply(&json!({"id":3,"method":"tools/call"}), &tools).is_none());
        assert!(reply(&json!({"method":"notifications/initialized"}), &tools).is_none());
        let unknown = reply(&json!({"id":4,"method":"resources/list"}), &tools).expect("error");
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn config_carries_pi_schemas_under_mcp_names() {
        let tools = [
            json!({"name":"bash","description":"Run","input_schema":{"type":"object","properties":{"command":{"type":"string"}}}}),
        ];
        let config: Value =
            serde_json::from_str(&pi_mcp_config(std::path::Path::new("/bin/adapter"), &tools))
                .expect("config json");
        let server = &config["mcpServers"]["pi"];
        assert_eq!(server["command"], "/bin/adapter");
        assert_eq!(server["args"], json!(["__pi-tools-mcp"]));
        let listed: Value =
            serde_json::from_str(server["env"]["CLAUDEX_PI_TOOLS"].as_str().expect("env"))
                .expect("tools");
        assert_eq!(
            listed[0]["inputSchema"]["properties"]["command"]["type"],
            "string"
        );
        assert_eq!(pi_allowed_tools(&tools), "mcp__pi__bash");
    }
}
