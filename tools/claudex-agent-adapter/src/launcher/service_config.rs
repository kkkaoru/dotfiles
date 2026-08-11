use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::SocketAddr,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};

use super::{
    AdapterOptions, LOCAL_TOKEN, installed_adapter, launcher_logs, program_identity,
    claude_relay::requires_authentication,
    daemon_arguments::{
        route_descriptions, search_worker_route_descriptions, worker_route_descriptions,
    },
};
use crate::{ADAPTER_PROTOCOL_VERSION, app_server};

#[derive(Debug)]
pub(crate) struct ServiceConfig {
    pub(crate) options: AdapterOptions,
    pub(crate) token: String,
    pub(crate) codex_config_fingerprint: String,
    pub(crate) service_config_fingerprint: String,
    pub(crate) executable: PathBuf,
    pub(crate) log_path: PathBuf,
    pub(crate) lock_path: PathBuf,
}

impl ServiceConfig {
    pub(crate) fn new(options: AdapterOptions) -> Result<Self> {
        let token = std::env::var("ANTHROPIC_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| LOCAL_TOKEN.to_owned());
        if requires_authentication(&options.listen, &token) {
            bail!("ANTHROPIC_AUTH_TOKEN is required for a non-loopback listener");
        }
        let executable = installed_adapter::resolve_service_executable(
            std::env::current_exe().context("locate adapter executable")?,
        );
        let home = std::env::var_os("HOME").context("HOME is required")?;
        let source_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".codex"));
        let codex_config_fingerprint = app_server::provider_config_fingerprint(&source_home);
        let service_config_fingerprint =
            service_config_fingerprint(&options, &codex_config_fingerprint);
        let cache = PathBuf::from(home).join(".cache/claudex");
        let log_path = launcher_logs::adapter_log_path(&cache, &options.listen);
        let lock_path = launcher_logs::adapter_lock_path(&cache, &options.listen);
        Ok(Self {
            options,
            token,
            codex_config_fingerprint,
            service_config_fingerprint,
            executable,
            log_path,
            lock_path,
        })
    }

    pub(crate) fn base_url(&self) -> String {
        let listen = match self.options.listen {
            SocketAddr::V4(address) if address.ip().is_unspecified() => {
                SocketAddr::from(([127, 0, 0, 1], address.port()))
            }
            SocketAddr::V6(address) if address.ip().is_unspecified() => {
                SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], address.port()))
            }
            listen => listen,
        };
        format!("http://{listen}")
    }

    pub(crate) fn with_listen(&self, listen: SocketAddr) -> Self {
        let cache = self.log_path.parent().expect("adapter log parent");
        let mut options = self.options.clone();
        options.listen = listen;
        Self {
            options,
            token: self.token.clone(),
            codex_config_fingerprint: self.codex_config_fingerprint.clone(),
            service_config_fingerprint: self.service_config_fingerprint.clone(),
            executable: self.executable.clone(),
            log_path: launcher_logs::adapter_log_path(cache, &listen),
            lock_path: launcher_logs::adapter_lock_path(cache, &listen),
        }
    }
}

fn service_config_fingerprint(options: &AdapterOptions, codex_fingerprint: &str) -> String {
    let mut hasher = DefaultHasher::new();
    ADAPTER_PROTOCOL_VERSION.hash(&mut hasher);
    codex_fingerprint.hash(&mut hasher);
    options.model.hash(&mut hasher);
    route_descriptions(&options.routes).hash(&mut hasher);
    worker_route_descriptions(&options.model_catalog).hash(&mut hasher);
    search_worker_route_descriptions(&options.model_catalog).hash(&mut hasher);
    program_identity::identity(&options.routes).hash(&mut hasher);
    options.subscription_max_processes.hash(&mut hasher);
    options.subscription_timeout_minutes.hash(&mut hasher);
    options
        .subagent_hard_timeout_seconds
        .map(std::num::NonZeroU64::get)
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
