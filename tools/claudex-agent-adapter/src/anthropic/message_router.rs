use std::sync::Arc;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{
    Bridge, MessagesRequest, RequestIdentity, request_routing, token_count, trace_request,
};

impl Bridge {
    pub async fn messages(self: &Arc<Self>, request: MessagesRequest) -> Result<Response<Body>> {
        let tools_were_provided = !request.tools.is_empty();
        self.messages_inner(request, tools_were_provided).await
    }

    /// Process a native Claude Code request with its transport identity.
    ///
    /// The identity is attached after body deserialization so a caller cannot
    /// spoof it through `metadata`. It remains available to context retries and
    /// every central SubAgent classifier for the lifetime of the request.
    pub async fn messages_with_identity(
        self: &Arc<Self>,
        mut request: MessagesRequest,
        identity: RequestIdentity,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        tracing::debug!(
            session_id = identity.session_id(),
            agent_id = identity.agent_id(),
            parent_agent_id = identity.parent_agent_id(),
            "received Claude Code transport identity"
        );
        self.tool_schemas
            .restore_or_remember(&identity, &mut request, tools_were_provided);
        identity.attach(&mut request);
        self.messages_inner(request, tools_were_provided).await
    }

    pub fn count_tokens_with_identity(
        &self,
        mut request: MessagesRequest,
        identity: &RequestIdentity,
        tools_were_provided: bool,
    ) -> usize {
        self.tool_schemas
            .restore_or_remember(identity, &mut request, tools_were_provided);
        token_count(&request)
    }

    async fn messages_inner(
        self: &Arc<Self>,
        mut request: MessagesRequest,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        trace_request(&request);
        self.schedule_idle_session_sweep();
        self.agent_efforts
            .retire_terminal_task_notifications(&request);
        if let Some(response) = self.async_agent_launch_handoff(&request).await {
            return Ok(response);
        }
        let intent = self
            .subagent_tool_continuation(&request)
            .await
            .unwrap_or_else(|| self.agent_efforts.take(&request));
        let is_subagent = intent.is_subagent;
        let route = request_routing::resolve_request_model_with_origin(
            &mut request,
            &self.model,
            intent.model_override,
            request_routing::RouteOrigin::new(
                is_subagent,
                intent.matched,
                intent.model_is_inherited,
            ),
            |model| {
                self.app.supports_model(model) || (self.legacy_main_route && model == self.model)
            },
            |model| self.model_catalog.matches(model),
        )?;
        let effort = self.resolve_request_effort(&request, intent.effort);
        tracing::debug!(
            request_model = %request.model,
            request_effort = ?effort,
            is_subagent,
            ?route,
            "resolved request routing"
        );
        if route == request_routing::RouteDecision::Subscription {
            return self
                .subscription_messages(request, effort, is_subagent, tools_were_provided)
                .await;
        }
        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        self.provider_messages(
            request,
            input_tokens,
            effort,
            is_subagent,
            intent.run_in_background,
        )
        .await
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn messages_wrapper_forwards_tool_presence_to_the_inner_router() {
        let root = tempfile::tempdir().expect("message wrapper fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("source home");
        std::fs::write(source.join("auth.json"), "{}").expect("source auth");
        let program = root.path().join("app-server");
        std::fs::write(
            &program,
            "#!/bin/sh\nwhile IFS= read -r line; do id=$(printf '%s\\n' \"$line\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p'); printf '{\"id\":%s,\"result\":{}}\\n' \"$id\"; done\n",
        )
        .expect("app-server fixture");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("make app-server fixture executable");
        let app = crate::app_server::AppServer::spawn_with_program(
            "main",
            &program,
            &source,
            &root.path().join("isolated"),
        )
        .await
        .expect("start app-server fixture");
        let bridge = std::sync::Arc::new(Bridge::new_with_backend(
            crate::agent_backend::AgentBackend::codex(app),
            "main".to_owned(),
        ));
        let request = MessagesRequest {
            model: "main".to_owned(),
            system: Value::Null,
            messages: vec![json!({"role":"user","content":"hello"})],
            tools: vec![json!({"name":"Read"})],
            stream: true,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(2), bridge.messages(request))
                .await
                .expect("message wrapper should not hang")
                .expect("streaming response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
