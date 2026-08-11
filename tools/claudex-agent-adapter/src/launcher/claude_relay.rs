use std::{ffi::OsString, io::Write, net::SocketAddr};

use anyhow::{Result, bail};

use super::{LOCAL_TOKEN, process_io::relay_filtered_io};

pub(super) fn requires_authentication(listen: &SocketAddr, token: &str) -> bool {
    !listen.ip().is_loopback() && token == LOCAL_TOKEN
}

pub(super) fn reject_model_override(arguments: &[OsString]) -> Result<()> {
    if arguments.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|argument| argument == "--model" || argument.starts_with("--model="))
    }) {
        bail!("pass the main model to adapter option --model, not to Claude Code arguments");
    }
    Ok(())
}

pub(super) fn relay_stderr(stderr: impl std::io::Read, model: &str) -> Result<()> {
    let mut output = std::io::stderr().lock();
    relay_filtered(stderr, model, &mut output)
}

pub(super) fn relay_filtered(
    mut input: impl std::io::Read,
    model: &str,
    output: &mut impl Write,
) -> Result<()> {
    relay_filtered_io(&mut input, model, output)
}
