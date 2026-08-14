use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::net::TcpStream;

use super::RebindState;

pub(super) async fn accept_or_backoff(
    result: std::io::Result<(TcpStream, SocketAddr)>,
) -> Option<(TcpStream, SocketAddr)> {
    match result {
        Ok(accepted) => Some(accepted),
        Err(error) => {
            tracing::error!(%error, "adapter accept failed");
            tokio::time::sleep(Duration::from_millis(10)).await;
            None
        }
    }
}

pub(super) fn ephemeral_bind_addr(canonical: SocketAddr) -> SocketAddr {
    match canonical.ip() {
        std::net::IpAddr::V4(_) => SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 0),
        std::net::IpAddr::V6(_) => SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), 0),
    }
}

pub(super) fn write_rebind_state(
    cache: &Path,
    canonical: SocketAddr,
    listen: SocketAddr,
) -> Result<()> {
    fs::create_dir_all(cache).context("create rebind state directory")?;
    let path = rebind_state_path(cache, &canonical);
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create rebind state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure rebind state")?;
    }
    output
        .write_all(
            &serde_json::to_vec(&RebindState {
                listen,
                pid: std::process::id(),
            })
            .context("encode rebind state")?,
        )
        .context("write rebind state")?;
    output.sync_all().context("sync rebind state")?;
    fs::rename(&temporary, path).context("publish rebind state")
}

pub(crate) fn rebind_state_path(cache: &Path, canonical: &SocketAddr) -> PathBuf {
    let token: String = canonical
        .to_string()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    cache.join(format!("rebind.{token}.json"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "listen_handover_support_tests.rs"]
mod tests;
