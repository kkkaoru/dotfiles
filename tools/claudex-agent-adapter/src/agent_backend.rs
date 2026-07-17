use std::{fmt, str::FromStr, sync::Arc};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    app_server::{AppServer, ThreadEvents},
    grok_acp::GrokAcp,
};

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

pub struct RoutedBackend {
    model: String,
    kind: BackendKind,
    startup: std::sync::OnceLock<tokio::sync::watch::Receiver<StartupState>>,
}

#[derive(Clone)]
enum StartupState {
    Starting,
    Ready(Result<Arc<AgentBackend>, Arc<str>>),
}

impl RoutedBackend {
    async fn get(&self) -> Result<Arc<AgentBackend>> {
        let mut startup = self.startup_receiver();
        loop {
            let state = startup.borrow_and_update().clone();
            match state {
                StartupState::Starting => startup
                    .changed()
                    .await
                    .context("backend startup task stopped without a result")?,
                StartupState::Ready(Ok(backend)) => return Ok(backend),
                StartupState::Ready(Err(error)) => bail!(error.to_string()),
            }
        }
    }

    fn startup_receiver(&self) -> tokio::sync::watch::Receiver<StartupState> {
        self.startup
            .get_or_init(|| {
                let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
                let kind = self.kind;
                let model = self.model.clone();
                tokio::spawn(async move {
                    let result = AgentBackend::spawn(kind, &model)
                        .await
                        .map_err(|error| Arc::<str>::from(format!("{error:#}")));
                    sender.send_replace(StartupState::Ready(result));
                });
                receiver
            })
            .clone()
    }

    fn ready_backend(&self) -> Option<Arc<AgentBackend>> {
        let state = self.startup.get()?.borrow().clone();
        match state {
            StartupState::Ready(Ok(backend)) => Some(backend),
            StartupState::Starting | StartupState::Ready(Err(_)) => None,
        }
    }

    fn is_alive(&self) -> bool {
        let Some(startup) = self.startup.get() else {
            return true;
        };
        match startup.borrow().clone() {
            StartupState::Starting => true,
            StartupState::Ready(Ok(backend)) => backend.is_alive(),
            StartupState::Ready(Err(_)) => false,
        }
    }
}

pub enum AgentBackend {
    Codex(Arc<AppServer>),
    Grok(Arc<GrokAcp>),
    Routed(Vec<RoutedBackend>),
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
        let backends = routes
            .iter()
            .map(|route| RoutedBackend {
                model: route.model.clone(),
                kind: route.backend,
                startup: std::sync::OnceLock::new(),
            })
            .collect::<Vec<_>>();
        Arc::new(Self::Routed(backends))
    }

    pub fn codex(server: Arc<AppServer>) -> Arc<Self> {
        Arc::new(Self::Codex(server))
    }

    pub fn grok(agent: Arc<GrokAcp>) -> Arc<Self> {
        Arc::new(Self::Grok(agent))
    }

    pub fn routed(routes: Vec<(String, Arc<Self>)>) -> Arc<Self> {
        Arc::new(Self::Routed(
            routes
                .into_iter()
                .map(|(model, backend)| {
                    let kind = backend.kind();
                    let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
                    sender.send_replace(StartupState::Ready(Ok(backend)));
                    let startup = std::sync::OnceLock::new();
                    assert!(startup.set(receiver).is_ok());
                    RoutedBackend {
                        model,
                        kind,
                        startup,
                    }
                })
                .collect(),
        ))
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
            Self::Routed(routes) => routes.iter().any(|route| route.model == model),
            Self::Codex(_) | Self::Grok(_) => false,
        }
    }

    pub fn route_descriptions(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes
                .iter()
                .map(|route| format!("{}={}", route.model, route.kind))
                .collect(),
            leaf => vec![leaf.kind().to_string()],
        }
    }

    pub fn models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes.iter().map(|route| route.model.clone()).collect(),
            Self::Codex(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        match self {
            Self::Routed(routes) => routes
                .iter()
                .filter(|route| route.ready_backend().is_some())
                .map(|route| route.model.clone())
                .collect(),
            Self::Codex(_) | Self::Grok(_) => vec![],
        }
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        match self {
            Self::Codex(server) => server.subscribe_thread(thread_id),
            Self::Grok(agent) => agent.subscribe_thread(thread_id),
            Self::Routed(routes) => {
                let (index, raw_id) = routed_thread(thread_id);
                routes[index]
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
            Self::Routed(routes) => routes.iter().all(RoutedBackend::is_alive),
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
                let index = routes
                    .iter()
                    .position(|route| route.model == model)
                    .with_context(|| {
                        format!("no backend route is configured for model `{model}`")
                    })?;
                let backend = routes[index].get().await?;
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

    pub async fn request_detached(self: &Arc<Self>, method: &str, params: Value) -> Result<()> {
        match self.as_ref() {
            Self::Codex(server) => server.request_detached(method, params).await,
            Self::Grok(agent) if method == "turn/start" => agent.start_turn(params).await,
            Self::Grok(_) => bail!("Grok ACP does not support backend request `{method}`"),
            Self::Routed(routes) if method == "turn/start" => {
                let thread_id = params
                    .get("threadId")
                    .and_then(Value::as_str)
                    .context("routed turn omitted threadId")?;
                let (index, raw_id) = routed_thread(thread_id);
                let mut routed_params = params.clone();
                routed_params["threadId"] = json!(raw_id);
                let backend = routes[index].get().await?;
                Box::pin(backend.request_detached(method, routed_params)).await
            }
            Self::Routed(_) => bail!("routed backend does not support request `{method}`"),
        }
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        match self {
            Self::Codex(server) => server.respond(id, result).await,
            Self::Grok(_) => bail!("Grok ACP did not request Claude Code tool result {id}"),
            Self::Routed(routes) => {
                let codex = routes
                    .iter()
                    .find(|route| route.kind == BackendKind::CodexAppServer)
                    .context("no Codex backend can accept a Claude Code tool result")?;
                let backend = codex
                    .ready_backend()
                    .context("Codex backend is not initialized for this tool result")?;
                Box::pin(backend.respond(id, result)).await
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
    use super::{AgentBackend, BackendKind, BackendRoute, RoutedBackend, StartupState};

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

        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        let startup = std::sync::OnceLock::new();
        startup.set(receiver).ok().unwrap();
        let starting = AgentBackend::Routed(vec![RoutedBackend {
            model: "starting".to_owned(),
            kind: BackendKind::CodexAppServer,
            startup,
        }]);
        assert!(starting.is_alive());
        drop(sender);
    }
}
