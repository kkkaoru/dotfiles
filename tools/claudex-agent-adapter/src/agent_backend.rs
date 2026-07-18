use std::{fmt, str::FromStr, sync::Arc};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    app_server::{AppServer, ThreadEvents},
    grok_acp::GrokAcp,
};

mod routes;

use routes::RoutedBackends;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    CodexAppServer,
    GrokAcp,
}

impl BackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexAppServer => "codex-app-server",
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
            "grok-acp" => Ok(Self::GrokAcp),
            _ => bail!("invalid backend `{value}`; expected codex-app-server or grok-acp"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendRoute {
    pub model: String,
    pub backend: BackendKind,
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
        Ok(Self {
            model: model.to_owned(),
            backend: backend.parse()?,
        })
    }
}

pub enum AgentBackend {
    Codex(Arc<AppServer>),
    Grok(Arc<GrokAcp>),
    Routed(RoutedBackends),
}

impl AgentBackend {
    pub async fn spawn(kind: BackendKind, model: &str) -> Result<Arc<Self>> {
        match kind {
            BackendKind::CodexAppServer => {
                Ok(Arc::new(Self::Codex(AppServer::spawn(model).await?)))
            }
            BackendKind::GrokAcp => Ok(Arc::new(Self::Grok(GrokAcp::spawn(model).await?))),
        }
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

    pub fn routed(routes: Vec<(String, Arc<Self>)>) -> Arc<Self> {
        Arc::new(Self::Routed(RoutedBackends::ready(routes)))
    }

    pub const fn kind(&self) -> BackendKind {
        match self {
            Self::Codex(_) => BackendKind::CodexAppServer,
            Self::Grok(_) => BackendKind::GrokAcp,
            Self::Routed(_) => panic!("a routed backend has no single kind"),
        }
    }

    pub fn supports_model(&self, model: &str) -> bool {
        match self {
            Self::Routed(routes) => routes.supports(model),
            Self::Codex(_) | Self::Grok(_) => false,
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
            Self::Codex(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.started_models(),
            Self::Codex(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        match self {
            Self::Codex(server) => server.subscribe_thread(thread_id),
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
            Self::Grok(agent) => agent.is_alive(),
            Self::Routed(routes) => routes.is_alive(),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        match self {
            Self::Codex(server) => server.request(method, params).await,
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

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        match self {
            Self::Codex(server) => server.respond(id, result).await,
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
mod tests {
    use super::{AgentBackend, BackendKind, BackendRoute};

    #[test]
    fn parses_and_displays_backend_kinds() {
        for (input, expected) in [
            ("codex-app-server", BackendKind::CodexAppServer),
            ("grok-acp", BackendKind::GrokAcp),
        ] {
            assert_eq!(input.parse::<BackendKind>().unwrap(), expected);
            assert_eq!(expected.to_string(), input);
        }
        assert!("unknown".parse::<BackendKind>().is_err());
        assert!("=grok-acp".parse::<BackendRoute>().is_err());
        assert_eq!(
            "model=grok-acp".parse::<BackendRoute>().unwrap(),
            BackendRoute {
                model: "model".to_owned(),
                backend: BackendKind::GrokAcp
            }
        );
        assert!("invalid".parse::<BackendRoute>().is_err());
        let routes = AgentBackend::spawn_routes(&[
            BackendRoute {
                model: "unused-codex".to_owned(),
                backend: BackendKind::CodexAppServer,
            },
            BackendRoute {
                model: "unused-grok".to_owned(),
                backend: BackendKind::GrokAcp,
            },
        ]);
        assert!(routes.started_models().is_empty());
        assert!(routes.is_alive());
        for model in ["gpt", "gpt-5.6-sol", "gpt_custom", "grok", "grok-4.5"] {
            assert!(
                routes.supports_model(model),
                "expected inferred route: {model}"
            );
        }
        for model in ["", "GPT-5.6-sol", "Grok-4.5", "claude-unconfigured"] {
            assert!(
                !routes.supports_model(model),
                "unexpected inferred route: {model}"
            );
        }
    }
}
