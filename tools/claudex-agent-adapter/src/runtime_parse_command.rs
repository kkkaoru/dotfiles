use std::{collections::VecDeque, ffi::OsString};

use anyhow::{Result, bail};

use super::command_helpers::{
    consume_separator, reject_inherit_model, reject_remaining, take_flag, utf8,
};
use super::parse_options::parse_options;
use super::RuntimeCommand;

pub(super) fn parse_command(mut arguments: VecDeque<OsString>) -> Result<RuntimeCommand> {
    let command = utf8(arguments.pop_front(), "command")?;
    match command.as_str() {
        "build-id" => {
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::BuildId)
        }
        "ensure" => {
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "ensure")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::Ensure(options.adapter))
        }
        "hot-swap" => {
            let wait_idle = take_flag(&mut arguments, "--wait-idle");
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "hot-swap")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::HotSwap(options.adapter, wait_idle))
        }
        "launch" => {
            let options = parse_options(&mut arguments)?;
            consume_separator(&mut arguments)?;
            let inherit_claude_model =
                options.inherit_claude_model || options.adapter.model.is_empty();
            Ok(RuntimeCommand::Launch(
                options.adapter,
                arguments.into(),
                inherit_claude_model,
            ))
        }
        "mcp-claudex-launch" => {
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::McpClaudexLaunch)
        }
        "serve" => {
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "serve")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::Serve(options.adapter))
        }
        _ => bail!(
            "unknown command `{command}`; expected build-id, ensure, hot-swap, launch, mcp-claudex-launch, or serve"
        ),
    }
}

