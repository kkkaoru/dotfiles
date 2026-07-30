use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    CodexAppServer,
    ConfiguredAcp,
    CopilotAcp,
    GrokAcp,
}

impl BackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexAppServer => "codex-app-server",
            Self::ConfiguredAcp => "configured-acp",
            Self::CopilotAcp => "copilot-acp",
            Self::GrokAcp => "grok-acp",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for BackendKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "codex-app-server" => Ok(Self::CodexAppServer),
            "configured-acp" => Ok(Self::ConfiguredAcp),
            "copilot-acp" => Ok(Self::CopilotAcp),
            "grok-acp" => Ok(Self::GrokAcp),
            _ => bail!(
                "invalid backend `{value}`; expected codex-app-server, configured-acp, copilot-acp, or grok-acp"
            ),
        }
    }
}
