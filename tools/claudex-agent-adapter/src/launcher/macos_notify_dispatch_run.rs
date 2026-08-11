use std::net::SocketAddr;

use anyhow::{Context, Result, bail};

use super::{Event, post_in_process};

pub(crate) fn run_internal(arguments: Vec<std::ffi::OsString>) -> Result<()> {
    let mut args = arguments.into_iter();
    let _argv0 = args.next();
    let flag = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing __internal-notify"))?;
    if flag.to_string_lossy() != "__internal-notify" {
        bail!("expected __internal-notify");
    }
    let kind = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing notify kind"))?;
    if kind.to_string_lossy() != "complete" {
        bail!("unsupported notify kind {}", kind.to_string_lossy());
    }
    let cache = std::path::PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("missing notify cache"))?,
    );
    let listen = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing notify listen"))?
        .to_string_lossy()
        .into_owned();
    let build_id = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing notify build_id"))?
        .to_string_lossy()
        .into_owned();
    let listen_addr: SocketAddr = listen.parse().context("parse notify listen")?;
    post_in_process(
        &cache,
        &listen_addr,
        Event::SwapComplete { listen, build_id },
    );
    Ok(())
}
