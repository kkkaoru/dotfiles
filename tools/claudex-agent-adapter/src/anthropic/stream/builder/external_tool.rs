use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{SegmentBuilder, ToolCall};
use crate::anthropic::stream::protocol::StreamSender;
use crate::anthropic::{Bridge, Session};

#[derive(Clone, Copy)]
pub(super) struct ExternalToolContext<'a> {
    pub(super) bridge: &'a Bridge,
    pub(super) session: &'a Session,
    pub(super) current_messages: &'a [Value],
    pub(super) system: &'a Value,
    pub(super) stream: Option<&'a StreamSender>,
}

pub(super) fn requested_external_tool_name<'a>(
    names: &'a HashMap<String, String>,
    provider_name: &str,
) -> Option<&'a str> {
    names.get(provider_name).map(String::as_str).or_else(|| {
        names
            .values()
            .find(|name| name.as_str() == provider_name)
            .map(String::as_str)
    })
}

pub(super) async fn reject_unrequested_tool(
    bridge: &Bridge,
    session: &Session,
    call: ToolCall,
) -> Result<()> {
    tracing::warn!(
        provider_tool_name = %call.name,
        "rejected a provider tool that Claude Code did not supply"
    );
    let (text, success) = unrequested_tool_reply(&call.name);
    bridge
        .app_for_session(session)
        .respond_for_model(
            &session.model,
            call.request_id,
            json!({
                "contentItems":[{"type":"inputText","text":text}],
                "success":success
            }),
        )
        .await
        .context("failed to reject an unrequested provider tool")
}

pub(in crate::anthropic) fn unrequested_tool_reply(name: &str) -> (String, bool) {
    if crate::anthropic::session::is_main_session_only_tool(name) {
        return (
            "Built-in advisor() is main-session only and was not executed. Continue the delegated task without it. Do not retry advisor(), and do not launch models listed in disabled_subagent_models.".to_owned(),
            true,
        );
    }
    (
        format!("Tool `{name}` was not supplied by Claude Code and was not executed."),
        false,
    )
}

fn hydrate_external_tool_arguments(
    context: &ExternalToolContext<'_>,
    original_name: &str,
    mut arguments: Value,
) -> Value {
    if !crate::anthropic::agent_effort::is_agent_tool(original_name) {
        return arguments;
    }
    crate::anthropic::agent_routing::hydrate_routing_fields_from_context(
        &mut arguments,
        context.current_messages,
        context.system,
        context.bridge.model_catalog(),
    );
    crate::anthropic::agent_routing::hydrate_standard_agent_to_parent(
        &mut arguments,
        &context.session.model,
        context.bridge.model_catalog(),
    );
    arguments
}

fn rewrite_reused_subagent_launch(
    context: ExternalToolContext<'_>,
    original_name: &str,
    arguments: &mut Value,
) {
    if !crate::anthropic::agent_effort::is_agent_tool(original_name) {
        return;
    }
    let Some(session_id) = context
        .session
        .claude_session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
    else {
        return;
    };
    let Some(recipient) = context
        .bridge
        .subagent_reuse
        .rewrite_launch_input(session_id, arguments)
    else {
        return;
    };
    tracing::info!(
        session_id,
        recipient,
        tool = original_name,
        "provider Agent/Task launch reused an existing SubAgent"
    );
}

fn skip_duplicate_subagent_launch(
    context: ExternalToolContext<'_>,
    original_name: &str,
    arguments: &Value,
    tool_use_id: &str,
) -> bool {
    if !crate::anthropic::agent_effort::is_agent_tool(original_name)
        || !crate::anthropic::subagent_reuse::reuse_enabled()
    {
        return false;
    }
    let Some(session_id) = context
        .session
        .claude_session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
    else {
        return false;
    };
    if arguments
        .get("resume")
        .and_then(Value::as_str)
        .is_some_and(|resume| !resume.is_empty())
    {
        return false;
    }
    if context
        .bridge
        .subagent_reuse
        .scope_is_occupied(session_id, arguments)
    {
        tracing::info!(
            session_id,
            tool = original_name,
            tool_use_id,
            "skipped duplicate same-scope same-model provider SubAgent launch"
        );
        return true;
    }
    if context
        .bridge
        .subagent_reuse
        .note_inflight_launch(session_id, arguments, tool_use_id)
    {
        return false;
    }
    tracing::info!(
        session_id,
        tool = original_name,
        tool_use_id,
        "skipped provider SubAgent launch because admission claim was taken"
    );
    true
}

impl SegmentBuilder {
    pub(super) async fn external_tool_call(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        call: ToolCall,
    ) -> Result<()> {
        let ToolCall {
            call_id,
            request_id,
            arguments,
            ..
        } = call;
        let reject_request_id = request_id.clone();
        let mut arguments = hydrate_external_tool_arguments(&context, original_name, arguments);
        rewrite_reused_subagent_launch(context, original_name, &mut arguments);
        // Apply the exact denylist before failover as well as after it.  The
        // first check prevents a stale exhausted launch from being rewritten
        // at all when the caller explicitly disabled that source model.  The
        // second check below is mandatory because quota failover can replace
        // `claudex_model` with a sibling that is independently disabled.
        if self
            .reject_disabled_subagent(
                context,
                original_name,
                &arguments,
                reject_request_id.clone(),
            )
            .await?
        {
            return Ok(());
        }
        if crate::anthropic::agent_effort::is_agent_tool(original_name) {
            context.bridge.rewrite_exhausted_agent_launch_with_quota(
                &mut arguments,
                context.current_messages,
                context.system,
            );
        }
        // Re-check after quota rewrite: exact membership, never a prefix or
        // provider-family match, remains authoritative for the final route.
        if self
            .reject_disabled_subagent(
                context,
                original_name,
                &arguments,
                reject_request_id.clone(),
            )
            .await?
        {
            return Ok(());
        }
        if self
            .reject_exhausted_subagent(
                context,
                original_name,
                &arguments,
                reject_request_id.clone(),
            )
            .await?
        {
            return Ok(());
        }
        if self
            .reject_unroutable_subagent(
                context,
                original_name,
                &arguments,
                reject_request_id.clone(),
            )
            .await?
        {
            return Ok(());
        }
        if self
            .reject_stale_task_output(context, original_name, &arguments, reject_request_id)
            .await?
        {
            return Ok(());
        }
        let tool_use_id = format!("toolu_{}", Uuid::new_v4().simple());
        if skip_duplicate_subagent_launch(context, original_name, &arguments, &tool_use_id) {
            return self
                .reject_duplicate_subagent(context, original_name, request_id)
                .await;
        }
        self.emit_external_tool_use(
            context,
            original_name,
            call_id,
            tool_use_id,
            request_id,
            arguments,
        )
        .await
    }
}

#[path = "external_tool_emit.rs"]
mod emit;
