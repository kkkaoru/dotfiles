use std::{net::SocketAddr, path::Path};

use super::macos_notify::{Event, post_in_process};

/// Opt-in gate for user-visible banners.
///
/// Defaults off so mcp/launch/`hot-swap --wait-idle` waiters stay silent.
/// CLI `ensure` / interactive `hot-swap` call [`opt_in_cli_swap_notify`] when
/// the env is unset. `claudex install` forces `0` during swap then posts one
/// `__internal-notify` itself to avoid promote + idle-replace double alerts.
pub(crate) const MACOS_NOTIFY_ENV: &str = "CLAUDEX_MACOS_NOTIFY";

/// Whether interactive CLI `ensure` / `hot-swap` should show a swap banner.
///
/// Unset defaults to on for CLI wrappers. Explicit `0` (after-install) stays off
/// so the wrapper can post exactly one `__internal-notify` itself.
pub(crate) fn cli_wants_swap_banner() -> bool {
    match std::env::var(MACOS_NOTIFY_ENV) {
        Err(_) => true,
        Ok(value) => parse_notify_env(Some(value.as_str())),
    }
}

/// Silence mid-replace banners so promote cannot pair with a post-swap notify.
pub(crate) fn silence_swap_banners_for_replace(banner: bool) {
    if !banner {
        return;
    }
    // CLI entry is single-threaded before ensure/hot-swap work begins.
    unsafe {
        std::env::set_var(MACOS_NOTIFY_ENV, "0");
    }
}

/// Enable banners and emit exactly one Complete for the current build_id.
/// Same build_id is deduped by [`super::macos_notify::should_emit_at`].
pub(crate) fn emit_cli_swap_complete_banner(config: &super::ServiceConfig) {
    unsafe {
        std::env::set_var(MACOS_NOTIFY_ENV, "1");
    }
    super::macos_notify::swap_complete(config);
}

/// Enable swap-complete banners for interactive CLI ensure/hot-swap.
///
/// Prefer [`cli_wants_swap_banner`] + [`silence_swap_banners_for_replace`] +
/// [`emit_cli_swap_complete_banner`] so replace stays silent until one final
/// Complete. Kept for unit coverage of the unset→1 / explicit-0 contract.
pub(crate) fn opt_in_cli_swap_notify() {
    if std::env::var_os(MACOS_NOTIFY_ENV).is_some() {
        return;
    }
    // CLI entry is single-threaded before ensure/hot-swap work begins.
    unsafe {
        std::env::set_var(MACOS_NOTIFY_ENV, "1");
    }
}

#[cfg(test)]
std::thread_local! {
    static TEST_FORCE_ENABLED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static TEST_FORCE_DELEGATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn notifications_enabled() -> bool {
    #[cfg(test)]
    {
        if let Some(forced) = TEST_FORCE_ENABLED.with(std::cell::Cell::get) {
            return forced;
        }
        // Ignore process env on the default test path so a parallel opt-out
        // suite cannot disable banners for unrelated threads.
        true
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
        // Production defaults off until CLI ensure/hot-swap or install opt in.
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

#[cfg(test)]
pub(super) struct DelegateForceGuard {
    previous: bool,
}

#[cfg(test)]
impl DelegateForceGuard {
    pub(super) fn push(enabled: bool) -> Self {
        let previous = TEST_FORCE_DELEGATE.with(std::cell::Cell::get);
        TEST_FORCE_DELEGATE.with(|cell| cell.set(enabled));
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for DelegateForceGuard {
    fn drop(&mut self) {
        TEST_FORCE_DELEGATE.with(|cell| cell.set(self.previous));
    }
}

pub(super) fn post(cache: &Path, listen: &SocketAddr, event: Event) {
    if delegate_post(cache, &event) {
        return;
    }
    post_in_process(cache, listen, event);
}

#[path = "macos_notify_dispatch_delegate.rs"]
mod delegate;
#[path = "macos_notify_dispatch_run.rs"]
mod run;
use delegate::delegate_post;
#[cfg(test)]
use delegate::{delegate_complete_notify, interpret_delegate_status};
pub(crate) use run::run_internal;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "macos_notify_dispatch_tests.rs"]
mod tests;
