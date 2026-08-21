use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Reserved so SubAgent bursts cannot starve interactive user turns.
const OUTER_TURN_RESERVE: usize = 1;
pub(crate) const MAX_MODEL_CONCURRENCY: usize =
    tokio::sync::Semaphore::MAX_PERMITS - OUTER_TURN_RESERVE;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    CodexAppServer,
    PiGateway,
}

impl<'de> Deserialize<'de> for BackendKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl BackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexAppServer => "codex-app-server",
            Self::PiGateway => "pi-gateway",
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
            "pi-gateway" => Ok(Self::PiGateway),
            "configured-acp" | "copilot-acp" | "grok-acp" => {
                bail!("invalid backend `{value}`; ACP backends are removed, use pi-gateway")
            }
            _ => bail!("invalid backend `{value}`; expected codex-app-server or pi-gateway"),
        }
    }
}
