use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub(super) fn record_tools_call(message: &Value) {
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

pub(super) fn record_tools_call_to(message: &Value, timestamp: f64, paths: impl IntoIterator<Item = PathBuf>) {
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

pub(super) fn write_message(stdout: &mut impl Write, ndjson: bool, message: Value) -> Result<()> {
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

pub(super) fn read_message(reader: &mut impl BufRead) -> Result<Option<(Value, bool)>> {
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
