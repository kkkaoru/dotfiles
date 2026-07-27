use std::sync::Arc;

use anyhow::{Context, Result};

use super::{AcpLaunch, AcpProvider, GrokAcp};

impl GrokAcp {
    pub async fn spawn_configured(model: &str, launch: &AcpLaunch) -> Result<Arc<Self>> {
        Self::spawn_configured_with_max_concurrency(model, launch, None).await
    }

    pub(crate) async fn spawn_configured_with_max_concurrency(
        model: &str,
        launch: &AcpLaunch,
        max_concurrency: Option<usize>,
    ) -> Result<Arc<Self>> {
        let cwd = std::env::current_dir().context("resolve configured ACP working directory")?;
        let provider = if launch
            .arguments
            .iter()
            .any(|argument| argument.contains("{model}"))
        {
            AcpProvider::ConfiguredLaunchScoped
        } else {
            AcpProvider::Configured
        };
        Self::spawn_provider(
            provider,
            model,
            &launch.program,
            Some(launch.arguments.clone()),
            cwd,
            max_concurrency,
        )
        .await
    }
}
