use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::{IpAddr, SocketAddr, TcpListener},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, daemon_process, daemon_start, handover, health::wait_until_ready};

const STATE_PREFIX: &str = "fallback.";
const STATE_SUFFIX: &str = ".json";

#[derive(Debug, Deserialize, Serialize)]
struct FallbackState {
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

    let listen = reserve_loopback_listen(config.options.listen)?;
    let fallback = config.with_listen(listen);
    let pid = daemon_start::start_adapter(&fallback).context("start current-build fallback")?;
    if let Err(error) = wait_until_ready(client, &fallback).await {
        if daemon_process::matches(pid, &fallback.executable) {
            daemon_process::terminate(pid);
        }
        return Err(error.context("wait for current-build fallback"));
    }
    write_state(
        &state_path,
        &FallbackState {
            listen,
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            service_config_fingerprint: fallback.service_config_fingerprint.clone(),
            pid,
        },
    )?;
    Ok(fallback.base_url())
}

pub(super) fn reserve_loopback_listen(configured: SocketAddr) -> Result<SocketAddr> {
    reserve_listener(configured)
}

fn reserve_listener(configured: SocketAddr) -> Result<SocketAddr> {
    let ip = match configured.ip() {
        IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    };
    TcpListener::bind(SocketAddr::new(ip, 0))
        .context("reserve current-build fallback listener")?
        .local_addr()
        .context("read current-build fallback listener")
}

fn state_path(config: &ServiceConfig) -> Result<PathBuf> {
    let parent = config
        .log_path
        .parent()
        .context("adapter log has no parent")?;
    Ok(parent.join(format!(
        "{STATE_PREFIX}{}{}",
        config.options.listen.port(),
        STATE_SUFFIX
    )))
}

fn read_state(path: &PathBuf) -> Result<Option<FallbackState>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: FallbackState =
        serde_json::from_slice(&fs::read(path).context("read current-build fallback state")?)
            .context("decode current-build fallback state")?;
    if !state.listen.ip().is_loopback() || state.listen.port() == 0 || state.pid == 0 {
        bail!("invalid current-build fallback state");
    }
    Ok(Some(state))
}

fn write_state(path: &PathBuf, state: &FallbackState) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create current-build fallback state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure current-build fallback state")?;
    }
    output
        .write_all(&serde_json::to_vec(state).context("encode current-build fallback state")?)
        .context("write current-build fallback state")?;
    output
        .sync_all()
        .context("sync current-build fallback state")?;
    fs::rename(&temporary, path).context("publish current-build fallback state")
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::path::PathBuf;

    use super::{FallbackState, read_state, reserve_listener, state_path, write_state};

    #[test]
    fn reserves_a_loopback_listener_for_wildcard_configuration() {
        let listen = reserve_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8318))
            .expect("fallback listener");
        assert!(listen.ip().is_loopback());
        assert_ne!(listen.port(), 0);
        let v6 = reserve_listener(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8318))
            .expect("ipv6 fallback listener");
        assert!(v6.ip().is_loopback());
    }

    #[test]
    fn fallback_state_keeps_the_generation_identity_and_port() {
        let state = FallbackState {
            listen: "127.0.0.1:8324".parse().unwrap(),
            build_id: "build".to_owned(),
            service_config_fingerprint: "service".to_owned(),
            pid: 42,
        };
        let encoded = serde_json::to_string(&state).unwrap();
        let decoded: FallbackState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.listen, state.listen);
        assert_eq!(decoded.build_id, state.build_id);
        assert_eq!(
            decoded.service_config_fingerprint,
            state.service_config_fingerprint
        );
        assert_eq!(decoded.pid, state.pid);
    }

    #[test]
    fn read_state_rejects_invalid_records_and_round_trips_valid_ones() {
        let root = tempfile::tempdir().expect("fallback state fixture");
        let path = root.path().join("fallback.8318.json");
        assert!(read_state(&path).expect("missing state").is_none());

        write_state(
            &path,
            &FallbackState {
                listen: "127.0.0.1:8325".parse().unwrap(),
                build_id: "build".to_owned(),
                service_config_fingerprint: "service".to_owned(),
                pid: 99,
            },
        )
        .expect("write valid state");
        let loaded = read_state(&path).expect("read valid").expect("present");
        assert_eq!(loaded.pid, 99);

        std::fs::write(
            &path,
            br#"{"listen":"8.8.8.8:80","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
        )
        .expect("non-loopback");
        assert!(read_state(&path).is_err());
        std::fs::write(
            &path,
            br#"{"listen":"127.0.0.1:0","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
        )
        .expect("port zero");
        assert!(read_state(&path).is_err());
        std::fs::write(
            &path,
            br#"{"listen":"127.0.0.1:80","build_id":"b","service_config_fingerprint":"s","pid":0}"#,
        )
        .expect("pid zero");
        assert!(read_state(&path).is_err());
    }

    #[test]
    fn state_path_uses_listen_port_beside_the_adapter_log() {
        let mut config = super::super::ServiceConfig {
            options: super::super::AdapterOptions {
                routes: vec![crate::agent_backend::BackendRoute::new(
                    "test-model",
                    crate::agent_backend::BackendKind::CodexAppServer,
                )],
                listen: "127.0.0.1:8318".parse().unwrap(),
                model: "test-model".to_owned(),
                subscription_max_processes: 20,
                subscription_timeout_minutes: 120,
                subagent_hard_timeout_seconds: None,
                model_catalog: crate::provider_config::ModelCatalog::default(),
            },
            token: super::super::LOCAL_TOKEN.to_owned(),
            codex_config_fingerprint: "test-fingerprint".to_owned(),
            service_config_fingerprint: "service-fingerprint".to_owned(),
            executable: PathBuf::from("/tmp/claudex-agent-adapter"),
            log_path: PathBuf::from("/tmp/claudex/adapter.log"),
            lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
        };
        let path = state_path(&config).expect("state path");
        assert!(path.ends_with("fallback.8318.json"));
        config.log_path = PathBuf::new();
        assert!(state_path(&config).is_err());
    }

    #[test]
    fn read_state_validates_port_and_ip() {
        let root = tempfile::tempdir().expect("validate port/ip fixture");
        let path = root.path().join("fallback.invalid.json");

        // Test port == 0 rejection
        std::fs::write(
            &path,
            br#"{"listen":"127.0.0.1:0","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
        )
        .expect("write port=0");
        assert!(read_state(&path).is_err());

        // Test non-loopback IP rejection
        std::fs::write(
            &path,
            br#"{"listen":"192.168.1.1:8000","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
        )
        .expect("write non-loopback");
        assert!(read_state(&path).is_err());
    }

    #[test]
    fn reserves_ipv6_loopback_listener() {
        let listen_v6 = reserve_listener(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 9000))
            .expect("ipv6 fallback listener");
        assert!(listen_v6.ip().is_loopback());
        assert!(matches!(listen_v6.ip(), IpAddr::V6(_)));
    }
}
