use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebSearchMode {
    #[default]
    DelegateCcr,
    CodexNative,
    AcpNative,
    DelegateMcp,
    Disabled,
}

impl WebSearchMode {
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::DelegateCcr)
    }
    /// Provider owns its agent loop and tools (Grok / OpenCode / Cursor ACP).
    /// Claude Code Agent/Task schemas are not executable on this path.
    pub const fn uses_provider_native_agent_loop(self) -> bool {
        matches!(self, Self::AcpNative | Self::DelegateMcp)
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DelegateCcr => "delegate-ccr",
            Self::CodexNative => "codex-native",
            Self::AcpNative => "acp-native",
            Self::DelegateMcp => "delegate-mcp",
            Self::Disabled => "disabled",
        }
    }
}

impl fmt::Display for WebSearchMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
