use std::net::{IpAddr, SocketAddr, TcpListener};

use anyhow::{Context, Result};

use crate::launcher::daemon_process;

pub(super) fn terminate_failed_fallback(pid: u32, executable: &std::path::Path) {
    if daemon_process::matches(pid, executable) {
        daemon_process::terminate(pid);
    }
}

pub(in crate::launcher) fn reserve_loopback_listen(configured: SocketAddr) -> Result<SocketAddr> {
    reserve_listener(configured)
}

pub(super) fn reserve_listener(configured: SocketAddr) -> Result<SocketAddr> {
    let ip = match configured.ip() {
        IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    };
    TcpListener::bind(SocketAddr::new(ip, 0))
        .context("reserve current-build fallback listener")?
        .local_addr()
        .context("read current-build fallback listener")
}
