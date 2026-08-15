use anyhow::Result;
use serde_json::{Value, json};

use super::super::tools::{
    build_developer_instructions, system_with_developer_instructions, thread_start_params_for_mode,
    tool_configuration_for_mode,
};
use crate::anthropic::{
    Bridge, MessagesRequest, Session,
    content::{serialized_len, system_text},
    request_identity,
    turn_input::{
        provider_turn_input, provider_turn_input_with_token_budget, provider_user_turn_input,
    },
};

impl Bridge {
    pub(super) async fn start_model_turn(
        &self,
        request: &MessagesRequest,
        session: &Session,
        existing_len: usize,
        extras: &[Value],
        effort: Option<&str>,
    ) -> Result<()> {
        let input = if existing_len == 0 {
            self.transcript_input(request)
        } else {
            provider_user_turn_input(&self.request_model(request), extras)
        };
        let mut params = json!({
            "threadId": session.thread_id,
            "input": input,
            "model": self.request_model(request)
        });
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        let provider = self.app_for_session(session);
        if provider.backend_kind_for_model(&session.model)
            == Some(crate::agent_backend::BackendKind::PiGateway)
        {
            let is_subagent = crate::anthropic::agent_effort::is_subagent_request(request);
            let acp_native = provider
                .web_search_mode(&session.model)
                .uses_provider_native_agent_loop();
            params["claudexRequest"] = pi_claude_request(request, is_subagent, acp_native)?;
        }
        // Mark interactive user turns so ACP keeps a reserved slot free of SubAgent load.
        if !crate::anthropic::agent_effort::is_subagent_request(request) {
            params["priority"] = json!("user");
        }
        self.app_for_session(session)
            .request_detached("turn/start", params)
            .await
    }

    fn transcript_input(&self, request: &MessagesRequest) -> Vec<Value> {
        let model = self.request_model(request);
        let provider = self.app_for(request_identity::claude_session_id(request).as_deref());
        if crate::command_code_acp::is_command_code_model(&model) {
            return provider_user_turn_input(&model, &request.messages);
        }
        let Some(limit) = provider.max_context_tokens_for_model(&model) else {
            return provider_turn_input(&model, &request.messages);
        };
        let web_search_mode = provider.web_search_mode(&model);
        let (dynamic_tools, _, _) =
            tool_configuration_for_mode(request, None, None, web_search_mode);
        let start_params =
            thread_start_params_for_mode(request, &model, dynamic_tools, web_search_mode);
        let setup_tokens = serialized_len(&start_params).div_ceil(4);
        let token_budget = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(setup_tokens);
        provider_turn_input_with_token_budget(&model, &request.messages, token_budget)
    }
}

pub(in crate::anthropic) fn pi_claude_request(
    request: &MessagesRequest,
    is_subagent: bool,
    acp_native: bool,
) -> Result<Value> {
    let developer_instructions = build_developer_instructions(request, is_subagent, acp_native);
    let combined_system =
        system_with_developer_instructions(&system_text(&request.system), &developer_instructions);
    let mut claude_request = serde_json::to_value(request)?;
    claude_request["system"] = json!(combined_system);
    Ok(claude_request)
}

pub(in crate::anthropic) fn is_context_window_exceeded(error: &anyhow::Error) -> bool {
    contains_context_window_marker(&error.to_string())
}

pub(in crate::anthropic) fn is_unknown_session_exceeded(error: &anyhow::Error) -> bool {
    is_unknown_session_text(&error.to_string())
}

pub(crate) fn is_unknown_session_text(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("quota")
        || lower.contains("usage limit")
        || lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        return false;
    }
    lower.contains("unknown session:") || lower.contains("unknown session")
}

pub(in crate::anthropic) fn contains_context_window_marker(message: &str) -> bool {
    let message = message.to_lowercase();
    const CONTEXT_WINDOW_MARKERS: [&str; 5] = [
        "context window",
        "ran out of room",
        "contextwindowexceeded",
        "context_window_exceeded",
        "context limit",
    ];
    CONTEXT_WINDOW_MARKERS
        .into_iter()
        .any(|marker| message.contains(marker))
}
