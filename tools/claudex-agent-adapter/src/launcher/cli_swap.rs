use anyhow::Result;

use super::{AdapterOptions, ServiceConfig, ensure, health, macos_notify_dispatch};

/// Interactive CLI `ensure`: silence mid-replace alerts, then at most one
/// Complete for this build_id. mcp/launch call [`super::ensure_running`] and stay quiet.
pub async fn ensure_running_cli(options: AdapterOptions) -> Result<String> {
    let banner = macos_notify_dispatch::cli_wants_swap_banner();
    macos_notify_dispatch::silence_swap_banners_for_replace(banner);
    let config = ServiceConfig::new(with_live_bootstrap_model(options).await?)?;
    let url = ensure::run(&config, ensure::Mode::Ensure).await?;
    if banner {
        macos_notify_dispatch::emit_cli_swap_complete_banner(&config);
    }
    Ok(url)
}

/// Interactive CLI `hot-swap`: one Complete after the swap attempt. Idle
/// waiters (`--wait-idle`) stay silent so after-install can post its own
/// single `__internal-notify`.
pub async fn hot_swap_cli(options: AdapterOptions, wait_idle: bool) -> Result<String> {
    let banner = !wait_idle && macos_notify_dispatch::cli_wants_swap_banner();
    macos_notify_dispatch::silence_swap_banners_for_replace(banner);
    let config = ServiceConfig::new(with_live_bootstrap_model(options).await?)?;
    let url = ensure::run(
        &config,
        if wait_idle {
            ensure::Mode::WaitIdle
        } else {
            ensure::Mode::HotSwap
        },
    )
    .await?;
    if banner {
        macos_notify_dispatch::emit_cli_swap_complete_banner(&config);
    }
    Ok(url)
}

/// Hot-swap / ensure often omit `--model`. Reuse then fails because health still
/// advertises the previous bootstrap model while the replacement starts empty.
async fn with_live_bootstrap_model(options: AdapterOptions) -> Result<AdapterOptions> {
    if !options.model.is_empty() {
        return Ok(options);
    }
    let probe = ServiceConfig::new(options.clone())?;
    let client = reqwest::Client::new();
    Ok(
        match health::fetch_health(&client, &probe).await {
            Some(health) if !health.model.is_empty() => AdapterOptions {
                model: health.model,
                ..options
            },
            _ => options,
        },
    )
}
