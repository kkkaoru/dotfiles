use std::{fs, net::SocketAddr};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, daemon_start, handover, health::wait_until_ready};

const STATE_PREFIX: &str = "fallback.";
const STATE_SUFFIX: &str = ".json";

mod state;
use state::{read_state, state_path, write_state};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct FallbackState {
    listen: SocketAddr,
    build_id: String,
    service_config_fingerprint: String,
    pid: u32,
}

/// Return a current-build listener when the configured listener is serving an older generation.
/// Existing requests remain attached to the old daemon; new Claude Code launches use this
/// listener instead of silently inheriting the old empty-response behavior.
pub(super) async fn ensure_current_generation(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<String> {
    let state_path = state_path(config)?;
    match read_state(&state_path) {
        Ok(Some(state)) => {
            let fallback = config.with_listen(state.listen);
            if state.build_id == env!("CLAUDEX_BUILD_ID")
                && state.service_config_fingerprint == fallback.service_config_fingerprint
                && matches!(
                    handover::inspect_service(client, &fallback).await,
                    handover::ServiceState::Reuse
                )
            {
                return Ok(fallback.base_url());
            }
            let _ = fs::remove_file(&state_path);
        }
        Ok(None) => {}
        Err(_) => {
            let _ = fs::remove_file(&state_path);
        }
    }

    // Reserving port 0 then releasing it before spawn races with parallel
    // listeners under the coverage suite. Retry a few times before failing.
    let mut last_error = None;
    for _ in 0..5 {
        let listen = reserve_loopback_listen(config.options.listen)?;
        let fallback = config.with_listen(listen);
        let pid = daemon_start::start_adapter(&fallback).context("start current-build fallback")?;
        match wait_until_ready(client, &fallback).await {
            Ok(()) => {
                write_state(
                    &state_path,
                    &FallbackState {
                        listen,
                        build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
                        service_config_fingerprint: fallback.service_config_fingerprint.clone(),
                        pid,
                    },
                )?;
                return Ok(fallback.base_url());
            }
            Err(error) => {
                terminate_failed_fallback(pid, &fallback.executable);
                last_error = Some(error);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("current-build fallback failed to start"))
        .context("wait for current-build fallback"))
}

#[path = "fallback_listen.rs"]
mod listen;
#[cfg(test)]
use listen::reserve_listener;
pub(super) use listen::reserve_loopback_listen;
use listen::terminate_failed_fallback;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "fallback_tests.rs"]
mod tests;
