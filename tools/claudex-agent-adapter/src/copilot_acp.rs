use std::{ffi::OsString, path::PathBuf, sync::Arc};

use anyhow::Result;
use serde_json::Value;

use crate::{app_server::ThreadEvents, grok_acp::GrokAcp};

/// GitHub Copilot CLI's ACP server, backed by the shared ACP transport driver.
pub struct CopilotAcp {
    inner: Arc<GrokAcp>,
}

impl CopilotAcp {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: GrokAcp::spawn_copilot(model).await?,
        }))
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: GrokAcp::spawn_copilot_with_program(model, program, cwd).await?,
        }))
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        self.inner.subscribe_thread(thread_id)
    }

    pub fn is_alive(&self) -> bool {
        self.inner.is_alive()
    }

    pub async fn create_session(&self, params: Value) -> Result<Value> {
        self.inner.create_session(params).await
    }

    pub async fn start_turn(&self, params: Value) -> Result<()> {
        self.inner.start_turn(params).await
    }

    pub async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        self.inner.cancel_turn(session_id).await
    }
}
