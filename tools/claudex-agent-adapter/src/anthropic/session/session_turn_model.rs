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
            // Ordinary Pi turns keep the Codex combined system. Command Code Luna/Spark
            // must not reuse that dump; they get worker-native instructions only.
            params["claudexRequest"] =
                pi_claude_request_for_model(request, is_subagent, false, &session.model)?;
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
        if crate::anthropic::is_command_code_model(&model) {
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

#[cfg(test)]
pub(in crate::anthropic) fn pi_claude_request(
    request: &MessagesRequest,
    is_subagent: bool,
    acp_native: bool,
) -> Result<Value> {
    pi_claude_request_for_model(request, is_subagent, acp_native, &request.model)
}

pub(in crate::anthropic) fn pi_claude_request_for_model(
    request: &MessagesRequest,
    is_subagent: bool,
    acp_native: bool,
    model: &str,
) -> Result<Value> {
    if crate::anthropic::is_command_code_model(model)
        || crate::anthropic::is_command_code_model(&request.model)
    {
        return command_code_pi_claude_request(request);
    }
    let developer_instructions = build_developer_instructions(request, is_subagent, acp_native);
    let combined_system =
        system_with_developer_instructions(&system_text(&request.system), &developer_instructions);
    let mut claude_request = serde_json::to_value(request)?;
    claude_request["system"] = json!(combined_system);
    // Nested SubAgent Pi turns must not receive Agent/Task launch tools.
    // SendMessage stays so a worker can continue an existing recipient.
    omit_hidden_launch_tools(
        &mut claude_request,
        is_subagent || !crate::anthropic::subagent_reuse::should_expose_launch_tools(request),
    );
    Ok(claude_request)
}

fn command_code_pi_claude_request(request: &MessagesRequest) -> Result<Value> {
    let developer_instructions = build_developer_instructions(request, true, true);
    let mut claude_request = serde_json::to_value(request)?;
    claude_request["system"] = json!(developer_instructions);
    claude_request["messages"] = json!(command_code_messages(&request.messages));
    // Pi gateway turns are stateless. Keep Claude Code's ordinary worker tools,
    // but hide coordination tools; command_code_messages converts returned
    // tool_result blocks into plain user context so a fresh provider thread
    // never receives a provider-owned tool call ID that it cannot recognize.
    // The outer coordinator owns SendMessage. Exposing it here makes a fresh
    // Command Code turn repeatedly notify main instead of reaching end_turn.
    omit_command_code_coordination_tools(&mut claude_request);
    Ok(claude_request)
}

fn command_code_messages(messages: &[Value]) -> Vec<Value> {
    let start = messages.iter().rposition(|message| {
        message.get("role").and_then(Value::as_str) == Some("user")
            && !content_has_tool_result(&message["content"])
            && !crate::anthropic::content::content_text(&message["content"]).is_empty()
    });
    let Some(start) = start else {
        return vec![json!({"role":"user", "content":command_code_result_context(messages)})];
    };
    let mut parts = Vec::new();
    let mut has_tool_results = false;
    for message in &messages[start..] {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let text = crate::anthropic::content::content_text(&message["content"]);
        if !text.is_empty() {
            parts.push(text);
        }
        has_tool_results |= content_has_tool_result(&message["content"]);
        append_tool_result_context(&message["content"], &mut parts);
    }
    if has_tool_results {
        parts.push(
            "[Tool execution status]\nDo not repeat a tool call whose result is listed above. Use the listed results. If the requested task is satisfied, return the final answer now; call only tools required for missing information."
                .to_owned(),
        );
    }
    vec![json!({"role":"user", "content":parts.join("\n\n")})]
}

fn command_code_result_context(messages: &[Value]) -> String {
    let mut parts = vec!["Continue the current task using these Claude tool results:".to_owned()];
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("user") {
            append_tool_result_context(&message["content"], &mut parts);
        }
    }
    parts.join("\n\n")
}

fn content_has_tool_result(content: &Value) -> bool {
    content.as_array().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
    })
}

fn append_tool_result_context(content: &Value, parts: &mut Vec<String>) {
    let Some(blocks) = content.as_array() else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let id = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let result = crate::anthropic::content::content_text(&block["content"]);
        let result = if result.is_empty() {
            block
                .get("content")
                .map_or_else(String::new, Value::to_string)
        } else {
            result
        };
        parts.push(format!("[Claude tool result {id}]\n{result}"));
    }
}

fn omit_command_code_coordination_tools(claude_request: &mut Value) {
    omit_hidden_launch_tools(claude_request, true);
    let Some(tools) = claude_request
        .get_mut("tools")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    tools.retain(|tool| tool.get("name").and_then(Value::as_str) != Some("SendMessage"));
}

fn omit_hidden_launch_tools(claude_request: &mut Value, hide_launch_tools: bool) {
    if !hide_launch_tools {
        return;
    }
    let Some(tools) = claude_request
        .get_mut("tools")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    tools.retain(|tool| {
        !tool
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(crate::anthropic::subagent_reuse::is_launch_tool)
    });
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
