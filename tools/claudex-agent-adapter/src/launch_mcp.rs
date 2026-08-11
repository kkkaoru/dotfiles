//! Stdio MCP server that exposes Claude Code Agent/Task launch tools.
//!
//! Cursor ACP attaches this as `claudex-launch`. Tool calls are acknowledged
//! here and also appended to a local queue so the adapter can bridge empty ACP
//! `providerTool` cards into Claude Code `tool_use`.

use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde_json::Value;

pub(super) const PROTOCOL_VERSION: &str = "2024-11-05";
pub(super) const SERVER_NAME: &str = "claudex-launch";
pub(super) const SERVER_VERSION: &str = "2.0.0";
const LAUNCH_QUEUE_FILE: &str = "launch-queue.jsonl";
mod message_io;
#[path = "launch_mcp_protocol.rs"]
mod protocol;
#[cfg(test)]
use message_io::record_tools_call_to;
use message_io::{read_message, record_tools_call, write_message};
use protocol::handle;
#[cfg(test)]
use protocol::tools;

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

#[cfg_attr(coverage_nightly, coverage(off))]
pub fn run_stdio() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    run_with_io(&mut stdin.lock(), &mut stdout)
}

pub(super) fn run_with_io(reader: &mut impl BufRead, stdout: &mut impl Write) -> Result<()> {
    let mut ndjson = false;
    loop {
        let Some((message, mode)) = read_message(reader)? else {
            break;
        };
        if mode {
            ndjson = true;
        }
        handle(&message, ndjson, stdout)?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "launch_mcp_tests.rs"]
mod tests;
