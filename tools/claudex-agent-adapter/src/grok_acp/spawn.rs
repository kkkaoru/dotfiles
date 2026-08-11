use std::{ffi::OsString, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};

use super::connection::AcpProvider;
use super::{DEFAULT_REASONING_EFFORT, GrokAcp};

#[path = "spawn_provider.rs"]
mod spawn_provider;
#[cfg(test)]
pub(in crate::grok_acp) use spawn_provider::session_create_capacity;

impl GrokAcp {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        Self::spawn_with_effort(model, DEFAULT_REASONING_EFFORT).await
    }

    pub async fn spawn_with_effort(model: &str, effort: &str) -> Result<Arc<Self>> {
        let program = std::env::var_os("CLAUDEX_GROK_PROGRAM").unwrap_or_else(|| "grok".into());
        let cwd = std::env::current_dir().context("resolve Grok ACP working directory")?;
        Self::spawn_provider(
            AcpProvider::Grok,
            model,
            Some(effort),
            program,
            None,
            cwd,
            None,
        )
        .await
    }

    pub async fn spawn_copilot(model: &str) -> Result<Arc<Self>> {
        let program =
            std::env::var_os("CLAUDEX_COPILOT_PROGRAM").unwrap_or_else(|| "copilot".into());
        let cwd = std::env::current_dir().context("resolve Copilot ACP working directory")?;
        Self::spawn_provider(AcpProvider::Copilot, model, None, program, None, cwd, None).await
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_with_program_and_effort(model, DEFAULT_REASONING_EFFORT, program, cwd).await
    }

    pub async fn spawn_with_program_and_effort(
        model: &str,
        effort: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(
            AcpProvider::Grok,
            model,
            Some(effort),
            program,
            None,
            cwd,
            None,
        )
        .await
    }

    pub async fn spawn_copilot_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(AcpProvider::Copilot, model, None, program, None, cwd, None).await
    }
}
