use std::{
    cell::RefCell,
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{connection::AcpProvider, prompt};
use crate::anthropic::subscription_request::cwd_from_system;

const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Task {
    pub(super) provider: AcpProvider,
    pub(super) connection: Rc<acp::ClientSideConnection>,
    pub(super) model: String,
    pub(super) cwd: PathBuf,
    pub(super) params: Value,
    pub(super) instructions: Rc<RefCell<HashMap<String, String>>>,
    pub(super) permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) response: oneshot::Sender<Result<Value>>,
}

impl Task {
    pub(super) fn spawn(self) {
        tokio::task::spawn_local(async move {
            let result = create(
                self.provider,
                &self.connection,
                &self.model,
                &self.cwd,
                self.params,
                &self.instructions,
            )
            .await;
            drop(self.permit);
            let _ = self.response.send(result);
        });
    }
}

pub(super) async fn create(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    model: &str,
    cwd: &Path,
    params: Value,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<Value> {
    // Claude Code embeds the active child cwd in its base instructions. Keep ACP sessions scoped
    // to that request instead of leaking the adapter daemon's launch directory.
    let session_cwd = session_cwd(&params, cwd);
    let request = connection.new_session(
        acp::NewSessionRequest::new(&session_cwd)
            .mcp_servers(vec![])
            .meta(json!({ "modelId": model }).as_object().cloned()),
    );
    let response = await_setup(provider, SESSION_SETUP_TIMEOUT, request).await?;
    // OpenCode currently ignores the non-standard modelId metadata on session/new. Select the
    // configured model through the ACP model method as soon as the session exists so the first
    // prompt cannot run against the provider default.
    if provider.is_session_scoped_configured() {
        await_model_setup(
            provider,
            SESSION_SETUP_TIMEOUT,
            connection.set_session_model(acp::SetSessionModelRequest::new(
                response.session_id.clone(),
                model.to_owned(),
            )),
        )
        .await?;
    }
    let session_id = response.session_id.0.to_string();
    let base = prompt::provider_instructions(&params, provider == AcpProvider::Grok);
    if !base.is_empty() {
        instructions.borrow_mut().insert(session_id.clone(), base);
    }
    Ok(json!({"thread":{"id":session_id}}))
}

fn session_cwd(params: &Value, fallback: &Path) -> PathBuf {
    params
        .get("baseInstructions")
        .and_then(Value::as_str)
        .and_then(cwd_from_system)
        .or_else(|| request_cwd(params))
        .unwrap_or_else(|| fallback.to_owned())
}

async fn await_model_setup<T>(
    provider: AcpProvider,
    timeout: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| {
            anyhow!(
                "{} ACP session/set_model timed out after {:?}",
                provider.label(),
                timeout
            )
        })?
        .map_err(|error| {
            anyhow!(
                "{} ACP session/set_model failed: {error:?}",
                provider.label()
            )
        })
}

async fn await_setup<T>(
    provider: AcpProvider,
    timeout: Duration,
    request: impl Future<Output = acp::Result<T>>,
) -> Result<T> {
    tokio::time::timeout(timeout, request)
        .await
        .map_err(|_| {
            anyhow!(
                "{} ACP session/new timed out after {:?}",
                provider.label(),
                timeout
            )
        })?
        .map_err(|error| anyhow!("{} ACP session/new failed: {error:?}", provider.label()))
}

fn request_cwd(params: &Value) -> Option<PathBuf> {
    params
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
}

#[cfg(test)]
// Coverage gates measure production ACP sessions; this inline module only contains tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounds_session_setup_and_reports_provider_failures() {
        let timeout = await_setup(
            AcpProvider::Configured,
            Duration::from_millis(1),
            std::future::pending::<acp::Result<()>>(),
        )
        .await
        .unwrap_err();
        assert!(timeout.to_string().contains("timed out"));
        let failed = await_setup(
            AcpProvider::Copilot,
            Duration::from_secs(1),
            std::future::ready(Err::<(), _>(acp::Error::internal_error())),
        )
        .await
        .unwrap_err();
        assert!(failed.to_string().contains("session/new failed"));
    }

    #[tokio::test]
    async fn bounds_model_setup_and_reports_provider_failures() {
        let timeout = await_model_setup(
            AcpProvider::Configured,
            Duration::from_millis(1),
            std::future::pending::<acp::Result<()>>(),
        )
        .await
        .unwrap_err();
        assert!(timeout.to_string().contains("session/set_model timed out"));

        let failed = await_model_setup(
            AcpProvider::Grok,
            Duration::from_secs(1),
            std::future::ready(Err::<(), _>(acp::Error::internal_error())),
        )
        .await
        .unwrap_err();
        assert!(failed.to_string().contains("session/set_model failed"));
    }

    #[test]
    fn accepts_only_existing_absolute_request_directories() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            request_cwd(&json!({"cwd":root.path()})),
            Some(root.path().to_owned())
        );
        assert!(request_cwd(&json!({"cwd":"relative"})).is_none());
        assert!(request_cwd(&json!({"cwd":"/definitely/missing"})).is_none());
        assert!(request_cwd(&Value::Null).is_none());
    }

    #[test]
    fn falls_back_from_invalid_system_and_request_directories() {
        let fallback = tempfile::tempdir().unwrap();
        let request = tempfile::tempdir().unwrap();
        assert_eq!(
            session_cwd(
                &json!({
                    "baseInstructions":"CWD: /definitely/missing",
                    "cwd":request.path()
                }),
                fallback.path(),
            ),
            request.path()
        );
        assert_eq!(
            session_cwd(
                &json!({
                    "baseInstructions":"CWD: relative/path",
                    "cwd":"/definitely/missing"
                }),
                fallback.path(),
            ),
            fallback.path()
        );
    }
}
