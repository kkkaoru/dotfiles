use std::{collections::VecDeque, ffi::OsString};

use anyhow::{Context, Result, bail};

use super::parse_options::ParsedOptions;

pub(super) fn reject_inherit_model(options: &ParsedOptions, command: &str) -> Result<()> {
    if options.inherit_claude_model {
        bail!("--inherit-claude-model is valid only for launch, not {command}");
    }
    Ok(())
}

pub(super) fn consume_separator(arguments: &mut VecDeque<OsString>) -> Result<()> {
    if arguments.front().and_then(|value| value.to_str()) == Some("--") {
        arguments.pop_front();
        return Ok(());
    }
    bail!("launch requires `--` before Claude Code arguments")
}

pub(super) fn reject_remaining(arguments: &VecDeque<OsString>) -> Result<()> {
    if arguments.is_empty() {
        return Ok(());
    }
    bail!("unexpected arguments after adapter options")
}

pub(super) fn take_flag(arguments: &mut VecDeque<OsString>, flag: &str) -> bool {
    arguments
        .iter()
        .position(|value| value == flag)
        .map(|index| {
            arguments.remove(index);
            true
        })
        .unwrap_or(false)
}

pub(super) fn utf8(value: Option<OsString>, name: &str) -> Result<String> {
    value
        .with_context(|| format!("{name} is required"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))
}
