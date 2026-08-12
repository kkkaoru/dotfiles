use std::sync::Arc;

use anyhow::Result;

use super::{AcpLaunch, AcpProvider, GrokAcp};
use crate::working_directory;

impl GrokAcp {
    pub async fn spawn_configured(model: &str, launch: &AcpLaunch) -> Result<Arc<Self>> {
        Self::spawn_configured_with_max_concurrency(model, launch, None, None).await
    }

    pub(crate) async fn spawn_configured_with_max_concurrency(
        model: &str,
        launch: &AcpLaunch,
        max_concurrency: Option<usize>,
        effort: Option<&str>,
    ) -> Result<Arc<Self>> {
        let cwd =
            working_directory::resolve_process_cwd("resolve configured ACP working directory")?;
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
            effort,
            &launch.program,
            Some(launch.arguments.clone()),
            cwd,
            max_concurrency,
        )
        .await
    }
}
