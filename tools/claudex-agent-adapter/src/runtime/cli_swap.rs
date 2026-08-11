use crate::launcher::{self, AdapterOptions};
use anyhow::Result;

/// Interactive `ensure`: one banner on successful replace. mcp/launch call
/// `ensure_running` directly and stay silent by default.
pub(super) async fn run_ensure(options: AdapterOptions) -> Result<String> {
    launcher::opt_in_cli_swap_notify();
    launcher::ensure_running(options).await
}

/// Interactive `hot-swap` opts into banners. Idle waiters (`--wait-idle`) are
/// spawned with `CLAUDEX_MACOS_NOTIFY` cleared and must stay silent so
/// after-install can post exactly one `__internal-notify`.
pub(super) async fn run_hot_swap(options: AdapterOptions, wait_idle: bool) -> Result<String> {
    if !wait_idle {
        launcher::opt_in_cli_swap_notify();
    }
    launcher::hot_swap(options, wait_idle).await
}
