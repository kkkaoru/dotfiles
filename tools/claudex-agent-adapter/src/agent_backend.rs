use std::{fmt, str::FromStr, sync::Arc};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    app_server::{AppServer, ThreadEvents},
    copilot_acp::CopilotAcp,
    grok_acp::GrokAcp,
};

mod routes;

use routes::RoutedBackends;

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AcpLaunch {
    pub program: String,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BackendRoute {
    pub model: String,
    pub backend: BackendKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_prefixes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp: Option<AcpLaunch>,
}

impl BackendRoute {
    pub fn new(model: impl Into<String>, backend: BackendKind) -> Self {
        Self {
            model: model.into(),
            backend,
            model_prefixes: Vec::new(),
            acp: None,
        }
    }

    pub fn description(&self) -> String {
        if self.model_prefixes.is_empty() && self.acp.is_none() {
            return format!("{}={}", self.model, self.backend);
        }
        serde_json::to_string(self).expect("backend route must serialize")
    }
}

impl FromStr for BackendRoute {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (model, backend) = value
            .split_once('=')
            .context("--backend-route must use MODEL=BACKEND")?;
        if model.is_empty() {
            bail!("--backend-route model must not be empty");
        }
        Ok(Self::new(model, backend.parse()?))
    }
}

pub enum AgentBackend {
    Codex(Arc<AppServer>),
    Copilot(Arc<CopilotAcp>),
    ConfiguredAcp(Arc<GrokAcp>),
    Grok(Arc<GrokAcp>),
    Routed(RoutedBackends),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TurnCancellation {
    Settled,
    Unsupported,
}

impl AgentBackend {
    pub async fn spawn(kind: BackendKind, model: &str) -> Result<Arc<Self>> {
        match kind {
            BackendKind::CodexAppServer => {
                Ok(Arc::new(Self::Codex(AppServer::spawn(model).await?)))
            }
            BackendKind::ConfiguredAcp => bail!("configured ACP launch details are required"),
            BackendKind::CopilotAcp => Ok(Arc::new(Self::Copilot(CopilotAcp::spawn(model).await?))),
            BackendKind::GrokAcp => Ok(Arc::new(Self::Grok(GrokAcp::spawn(model).await?))),
        }
    }

    async fn spawn_route(route: &BackendRoute) -> Result<Arc<Self>> {
        if let Some(acp) = &route.acp {
            return Ok(Arc::new(Self::ConfiguredAcp(
                GrokAcp::spawn_configured(&route.model, acp).await?,
            )));
        }
        Self::spawn(route.backend, &route.model).await
    }

    pub fn spawn_routes(routes: &[BackendRoute]) -> Arc<Self> {
        Arc::new(Self::Routed(RoutedBackends::lazy(routes)))
    }

    pub fn codex(server: Arc<AppServer>) -> Arc<Self> {
        Arc::new(Self::Codex(server))
    }

    pub fn grok(agent: Arc<GrokAcp>) -> Arc<Self> {
        Arc::new(Self::Grok(agent))
    }

    pub fn copilot(agent: Arc<CopilotAcp>) -> Arc<Self> {
        Arc::new(Self::Copilot(agent))
    }

    pub fn configured_acp(agent: Arc<GrokAcp>) -> Arc<Self> {
        Arc::new(Self::ConfiguredAcp(agent))
    }

    pub fn routed(routes: Vec<(String, Arc<Self>)>) -> Arc<Self> {
        Arc::new(Self::Routed(RoutedBackends::ready(routes)))
    }

    pub const fn kind(&self) -> BackendKind {
        match self {
            Self::Codex(_) => BackendKind::CodexAppServer,
            Self::ConfiguredAcp(_) => BackendKind::ConfiguredAcp,
            Self::Copilot(_) => BackendKind::CopilotAcp,
            Self::Grok(_) => BackendKind::GrokAcp,
            Self::Routed(_) => panic!("a routed backend has no single kind"),
        }
    }

    pub fn supports_model(&self, model: &str) -> bool {
        match self {
            Self::Routed(routes) => routes.supports(model),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => false,
        }
    }

    pub fn route_descriptions(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.descriptions(),
            leaf => vec![leaf.kind().to_string()],
        }
    }

    pub fn models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.models(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.started_models(),
            Self::Codex(_) | Self::ConfiguredAcp(_) | Self::Copilot(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        match self {
            Self::Codex(server) => server.subscribe_thread(thread_id),
            Self::Copilot(agent) => agent.subscribe_thread(thread_id),
            Self::ConfiguredAcp(agent) => agent.subscribe_thread(thread_id),
            Self::Grok(agent) => agent.subscribe_thread(thread_id),
            Self::Routed(routes) => {
                let (index, raw_id) = routed_thread(thread_id);
                routes
                    .route(index)
                    .ready_backend()
                    .expect("thread route backend must already be initialized")
                    .subscribe_thread(raw_id)
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            Self::Codex(server) => server.is_alive(),
            Self::Copilot(agent) => agent.is_alive(),
            Self::ConfiguredAcp(agent) => agent.is_alive(),
            Self::Grok(agent) => agent.is_alive(),
            Self::Routed(routes) => routes.is_alive(),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        match self {
            Self::Codex(server) => server.request(method, params).await,
            Self::Copilot(agent) if method == "thread/start" => agent.create_session(params).await,
            Self::Copilot(_) => bail!("Copilot ACP does not support backend request `{method}`"),
            Self::ConfiguredAcp(agent) if method == "thread/start" => {
                agent.create_session(params).await
            }
            Self::ConfiguredAcp(_) => {
                bail!("configured ACP does not support backend request `{method}`")
            }
            Self::Grok(agent) if method == "thread/start" => agent.create_session(params).await,
            Self::Grok(_) => bail!("Grok ACP does not support backend request `{method}`"),
            Self::Routed(routes) if method == "thread/start" => {
                let model = params
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let (index, route) = routes.resolve(model)?;
                let backend = route.get().await?;
                let mut response = Box::pin(backend.request(method, params)).await?;
                let raw_id = response
                    .pointer("/thread/id")
                    .and_then(Value::as_str)
                    .context("backend response omitted thread id")?;
                response["thread"]["id"] = json!(format!("{index}:{raw_id}"));
                Ok(response)
            }
            Self::Routed(_) => bail!("routed backend does not support request `{method}`"),
        }
    }

    pub async fn request_detached(self: &Arc<Self>, method: &str, mut params: Value) -> Result<()> {
        match self.as_ref() {
            Self::Codex(server) => server.request_detached(method, params).await,
            Self::Copilot(agent) if method == "turn/start" => agent.start_turn(params).await,
            Self::Copilot(_) => bail!("Copilot ACP does not support backend request `{method}`"),
            Self::ConfiguredAcp(agent) if method == "turn/start" => agent.start_turn(params).await,
            Self::ConfiguredAcp(_) => {
                bail!("configured ACP does not support backend request `{method}`")
            }
            Self::Grok(agent) if method == "turn/start" => agent.start_turn(params).await,
            Self::Grok(_) => bail!("Grok ACP does not support backend request `{method}`"),
            Self::Routed(routes) if method == "turn/start" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .context("routed turn omitted threadId")?
                    .to_owned();
                let (index, raw_id) = routed_thread(&thread_id);
                params["threadId"] = json!(raw_id);
                let backend = routes.route(index).get().await?;
                Box::pin(backend.request_detached(method, params)).await
            }
            Self::Routed(_) => bail!("routed backend does not support request `{method}`"),
        }
    }

    pub(crate) async fn cancel_turn(&self, thread_id: &str) -> Result<TurnCancellation> {
        match self {
            Self::Codex(_) => Ok(TurnCancellation::Unsupported),
            Self::Copilot(agent) => {
                agent.cancel_turn(thread_id).await?;
                Ok(TurnCancellation::Settled)
            }
            Self::ConfiguredAcp(agent) => {
                agent.cancel_turn(thread_id).await?;
                Ok(TurnCancellation::Settled)
            }
            Self::Grok(agent) => {
                agent.cancel_turn(thread_id).await?;
                Ok(TurnCancellation::Settled)
            }
            Self::Routed(routes) => {
                let (index, raw_id) = routed_thread(thread_id);
                let backend = routes
                    .route(index)
                    .ready_backend()
                    .context("thread route backend is unavailable during cancellation")?;
                Box::pin(backend.cancel_turn(raw_id)).await
            }
        }
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        match self {
            Self::Codex(server) => server.respond(id, result).await,
            Self::Copilot(_) => bail!("Copilot ACP did not request Claude Code tool result {id}"),
            Self::ConfiguredAcp(_) => {
                bail!("configured ACP did not request Claude Code tool result {id}")
            }
            Self::Grok(_) => bail!("Grok ACP did not request Claude Code tool result {id}"),
            Self::Routed(routes) => {
                let backend = routes
                    .first_ready(BackendKind::CodexAppServer)
                    .context("Codex backend is not initialized for this tool result")?;
                Box::pin(backend.respond(id, result)).await
            }
        }
    }

    pub async fn respond_for_model(&self, model: &str, id: Value, result: Value) -> Result<()> {
        match self {
            Self::Codex(server) => server.respond(id, result).await,
            Self::Copilot(_) => bail!("Copilot ACP did not request Claude Code tool result {id}"),
            Self::ConfiguredAcp(_) => {
                bail!("configured ACP did not request Claude Code tool result {id}")
            }
            Self::Grok(_) => bail!("Grok ACP did not request Claude Code tool result {id}"),
            Self::Routed(routes) => {
                let route = routes
                    .find(model)
                    .with_context(|| format!("no active backend route for model `{model}`"))?;
                let backend = route
                    .ready_backend()
                    .with_context(|| format!("backend for model `{model}` is not initialized"))?;
                Box::pin(backend.respond_for_model(model, id, result)).await
            }
        }
    }
}

fn routed_thread(thread_id: &str) -> (usize, &str) {
    let (index, raw_id) = thread_id
        .split_once(':')
        .expect("routed backend thread ID is missing its route prefix");
    (index.parse().expect("invalid routed backend index"), raw_id)
}

#[cfg(test)]
include!("agent_backend_tests.rs");
