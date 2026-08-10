use std::{net::SocketAddr, path::Path};

use anyhow::{Context, Result, bail};

use super::macos_notify::{Event, post_in_process};

/// Opt-in gate for user-visible banners. ensure/mcp stay silent unless
/// `claudex install` / `claudex-hot-swap` export this.
pub(crate) const MACOS_NOTIFY_ENV: &str = "CLAUDEX_MACOS_NOTIFY";

#[cfg(test)]
std::thread_local! {
    static TEST_FORCE_ENABLED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub(super) fn notifications_enabled() -> bool {
    #[cfg(test)]
    {
        if let Some(forced) = TEST_FORCE_ENABLED.with(std::cell::Cell::get) {
            return forced;
        }
        // Ignore process env on the default test path so a parallel opt-out
        // suite cannot disable banners for unrelated threads.
        return true;
    }
    #[cfg(not(test))]
    parse_notify_env(
        std::env::var_os(MACOS_NOTIFY_ENV)
            .as_deref()
            .and_then(|value| value.to_str()),
    )
}

pub(super) fn parse_notify_env(value: Option<&str>) -> bool {
    match value {
        Some("0" | "false" | "FALSE" | "no" | "NO") => false,
        Some("1" | "true" | "TRUE" | "yes" | "YES") => true,
        // Unit tests default on so notify coverage stays independent of host env.
        // Production defaults off until install/hot-swap opt in.
        _ => cfg!(test),
    }
}

#[cfg(test)]
pub(super) struct NotifyForceGuard {
    previous: Option<bool>,
}

#[cfg(test)]
impl NotifyForceGuard {
    pub(super) fn push(enabled: bool) -> Self {
        let previous = TEST_FORCE_ENABLED.with(std::cell::Cell::get);
        TEST_FORCE_ENABLED.with(|cell| cell.set(Some(enabled)));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for NotifyForceGuard {
    fn drop(&mut self) {
        TEST_FORCE_ENABLED.with(|cell| cell.set(self.previous));
    }
}

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "macos_notify_dispatch_tests.rs"]
mod tests;
