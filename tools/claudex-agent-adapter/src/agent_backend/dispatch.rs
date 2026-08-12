use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::sync::Arc;

use super::{
    AgentBackend, BackendKind,
    request::{request_session, routed_thread},
};

impl AgentBackend {
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
                let params = route.thread_start_params(params);
                let mut response = request_session(&route, method, params).await?;
                let raw_id = response
                    .pointer("/thread/id")
                    .and_then(Value::as_str)
                    .context("backend response omitted thread id")?;
                response["thread"]["id"] = json!(format!("{index}:{raw_id}"));
                Ok(response)
            }
            Self::Routed(_) => bail!("routed backend does not support request `{method}`"),
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().request(method, params)).await
            }
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
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().request_detached(method, params)).await
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
            Self::SessionScoped(scopes) => {
                Box::pin(scopes.unguarded_scope().respond(id, result)).await
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
            Self::SessionScoped(scopes) => {
                let backend = scopes
                    .unique_started_pool_for_model(model)
                    .unwrap_or_else(|| scopes.unguarded_scope());
                Box::pin(backend.respond_for_model(model, id, result)).await
            }
        }
    }
}
