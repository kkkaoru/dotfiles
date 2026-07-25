use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use super::{fetch_health, ServiceConfig};

pub(crate) async fn wait_until_stale_drains(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Result<()> {
    let deadline = Instant::now() + super::START_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            let pid = fetch_health(client, config).await.and_then(|health| health.pid);
            anyhow::bail!(
                "agent adapter is still draining prior sessions on pid {pid:?}; retry after sessions finish"
            );
        }
        if let Some(health) = fetch_health(client, config).await {
            if health.session_slots_used == 0 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        return Ok(());
    }
}

pub(crate) async fn wait_until_stopped_with(
    timeout: Duration,
    mut process_running: impl FnMut() -> bool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while process_running() {
        if Instant::now() >= deadline {
            bail!("agent adapter is still draining active requests; retry after they complete");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
