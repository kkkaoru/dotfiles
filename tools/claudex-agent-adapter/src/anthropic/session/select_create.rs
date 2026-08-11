use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::Result;
use serde_json::Value;
use tokio::sync::Mutex;

use super::super::super::{Bridge, MessagesRequest, Session, request_identity};
use super::super::tools::{thread_start_params_for_mode, tool_configuration_for_mode};
use crate::app_server::response_thread_id;

impl Bridge {
    pub(in crate::anthropic) async fn create_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Result<Arc<Session>> {
        let slot = self.acquire_session_slot().await?;
        let model = self.request_model(request);
        let web_search_mode = self.app.web_search_mode(&model);
        let (dynamic_tools, external_tool_names, _internal_tools) = tool_configuration_for_mode(
            request,
            advisor_model,
            collaborator_model,
            web_search_mode,
        );
        let params = thread_start_params_for_mode(request, &model, dynamic_tools, web_search_mode);
        let result = self.app.request("thread/start", params).await?;
        let session = Arc::new(Session {
            thread_id: response_thread_id(&result)?,
            model,
            disabled_subagent_models: request.disabled_subagent_models.clone(),
            signature,
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(HashSet::new()),
            external_tool_names,
            client_user_id: request
                .metadata
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            claude_session_id: request_identity::claude_session_id(request),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            _slot: slot,
        });
        self.sessions.lock().await.push(Arc::clone(&session));
        Ok(session)
    }
}
