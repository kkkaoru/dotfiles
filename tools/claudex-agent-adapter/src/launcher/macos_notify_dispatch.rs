use std::{net::SocketAddr, path::Path};

use anyhow::{Context, Result, bail};

use super::macos_notify::{Event, post_in_process};

pub(super) fn post(cache: &Path, listen: &SocketAddr, event: Event) {
    if delegate_post(cache, &event) {
        return;
    }
    post_in_process(cache, listen, event);
}

fn delegate_post(cache: &Path, event: &Event) -> bool {
    #[cfg(test)]
    {
        let _ = (cache, event);
        false
    }
    #[cfg(not(test))]
    {
        use std::process::Command;

        use super::{
            installed_adapter,
            macos_notify::NotifyKind,
        };

        if event.kind() != NotifyKind::Complete {
            return false;
        }
        let Some(exe) = installed_adapter::notify_delegate_executable() else {
            return false;
        };
        let Some(cache) = cache.to_str() else {
            return false;
        };
        match Command::new(&exe)
            .env(installed_adapter::NOTIFY_IN_PROCESS_ENV, "1")
            .args([
                "__internal-notify",
                "complete",
                cache,
                event.listen(),
                event.build_id(),
            ])
            .status()
        {
            Ok(status) if status.success() => true,
            Ok(status) => {
                eprintln!("claudex: delegated macOS notify exited {status}");
                false
            }
            Err(error) => {
                eprintln!("claudex: delegated macOS notify failed ({error})");
                false
            }
        }
    }
}

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
