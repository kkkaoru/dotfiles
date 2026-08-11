use std::{
    net::TcpStream,
    time::{Duration, Instant},
};

use anyhow::Result;

use super::daemon_process::{
    matches as process_matches, request_graceful_shutdown, terminate as force_terminate,
};
use super::{ServiceConfig, authenticates, fetch_health};

const LISTENER_RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ServiceState {
    Reuse,
    Defer {
        pid: Option<u32>,
        active_http_requests: usize,
        active_provider_turns: usize,
        active_subagents: usize,
    },
    Replace {
        pid: Option<u32>,
        recovery_generation: Option<String>,
    },
    Start,
}

#[cfg(not(test))]
const HOT_SWAP_DRAIN_TIMEOUT: Duration = Duration::from_secs(45);
#[cfg(test)]
const HOT_SWAP_DRAIN_TIMEOUT: Duration = Duration::from_millis(0);
const HOT_SWAP_DRAIN_POLL: Duration = Duration::from_millis(100);

pub(super) async fn inspect_service(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> ServiceState {
    inspect_service_with(client, config).await
}

pub(super) async fn inspect_service_with(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> ServiceState {
    let Some(health) = fetch_health(client, config).await else {
        return if listener_is_bound(config) {
            ServiceState::Defer {
                pid: None,
                active_http_requests: 0,
                active_provider_turns: 0,
                active_subagents: 0,
            }
        } else {
            ServiceState::Start
        };
    };
    if config.matches(&health)
        && health.build_id == env!("CLAUDEX_BUILD_ID")
        && authenticates(client, config).await
    {
        ServiceState::Reuse
    } else if health.status == "ok" && health.has_active_work() {
        // Never tear down a generation that is still serving a request.
        // Idle listeners, including ones with a launch TUI attached, are
        // replaced in place so `claudex` / `ensure` / `hot-swap` pick up a
        // newly installed binary without a fallback port.
        ServiceState::Defer {
            pid: health.pid,
            active_http_requests: health.active_http_requests,
            active_provider_turns: health.active_provider_turns,
            active_subagents: health.active_subagent_count(),
        }
    } else {
        ServiceState::Replace {
            pid: health.pid,
            recovery_generation: health.recovery_generation,
        }
    }
}

fn listener_is_bound(config: &ServiceConfig) -> bool {
    TcpStream::connect_timeout(&config.options.listen, Duration::from_millis(100)).is_ok()
}

pub(super) async fn wait_for_hot_swap_idle(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<ServiceState> {
    let deadline = Instant::now() + HOT_SWAP_DRAIN_TIMEOUT;
    loop {
        let state = inspect_service_with(client, config).await;
        match &state {
            ServiceState::Defer { .. } if Instant::now() < deadline => {
                tokio::time::sleep(HOT_SWAP_DRAIN_POLL).await;
            }
            _ => return Ok(state),
        }
    }
}


#[path = "handover_release.rs"]
mod release;
pub(super) use release::release_stale_listener;
#[cfg(test)]
pub(super) use release::release_stale_listener_with;
