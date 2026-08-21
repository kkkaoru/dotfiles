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
    let session_id = context.session.claude_session_id.as_deref().unwrap_or("");
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

async fn reject_non_follow_up_launch(
    builder: &mut SegmentBuilder,
    context: ExternalToolContext<'_>,
    original_name: &str,
    arguments: &mut Value,
    reject_request_id: Value,
    follow_up: bool,
) -> Result<bool> {
    if follow_up {
        return Ok(false);
    }
    if builder
        .reject_nested_subagent_launch(context, original_name, arguments, reject_request_id.clone())
        .await?
    {
        return Ok(true);
    }
    if builder
        .reject_capped_subagent_launch(context, original_name, reject_request_id.clone())
        .await?
    {
        return Ok(true);
    }
    if builder
        .reject_disabled_subagent(context, original_name, arguments, reject_request_id.clone())
        .await?
    {
        return Ok(true);
    }
    if crate::anthropic::agent_effort::is_agent_tool(original_name) {
        context.bridge.rewrite_exhausted_agent_launch_with_quota(
            arguments,
            context.current_messages,
            context.system,
        );
    }
    if builder
        .reject_disabled_subagent(context, original_name, arguments, reject_request_id.clone())
        .await?
    {
        return Ok(true);
    }
    if builder
        .reject_exhausted_subagent(context, original_name, arguments, reject_request_id.clone())
        .await?
    {
        return Ok(true);
    }
    builder
        .reject_unroutable_subagent(context, original_name, arguments, reject_request_id)
        .await
}

enum DuplicateSkip {
    Forward,
    Queued,
    Rejected,
}

const QUEUED_FOLLOW_UP_ACK: &str = "A same-scope SubAgent is already running, so this follow-up was queued for SendMessage({to}) once the worker id is known.";

async fn ack_queued_subagent_follow_up(
    builder: &mut SegmentBuilder,
    context: ExternalToolContext<'_>,
    original_name: &str,
    request_id: Value,
) -> Result<()> {
    tracing::info!(
        session_id = ?context.session.claude_session_id,
        tool_name = original_name,
        "queued duplicate provider SubAgent follow-up"
    );
    context
        .bridge
        .app_for_session(context.session)
        .respond_for_model(
            &context.session.model,
            request_id,
            json!({
                "contentItems":[{"type":"inputText","text":QUEUED_FOLLOW_UP_ACK}],
                "success":true
            }),
        )
        .await
        .context("failed to acknowledge a queued provider SubAgent follow-up")?;
    builder.suppressed_tool_use = true;
    Ok(())
}

fn skip_duplicate_subagent_launch(
    context: ExternalToolContext<'_>,
    original_name: &str,
    arguments: &mut Value,
    tool_use_id: &str,
) -> DuplicateSkip {
    if !crate::anthropic::agent_effort::is_agent_tool(original_name)
        || !crate::anthropic::subagent_reuse::reuse_enabled()
    {
        return DuplicateSkip::Forward;
    }
    let Some(session_id) = context
        .session
        .claude_session_id
        .as_deref()
        .filter(|session_id| !session_id.is_empty())
    else {
        return DuplicateSkip::Forward;
    };
    if crate::anthropic::subagent_reuse::is_send_message_follow_up(arguments) {
        return DuplicateSkip::Forward;
    }
    if context
        .bridge
        .subagent_reuse
        .scope_is_occupied(session_id, arguments)
    {
        return occupied_duplicate_skip(context, session_id, original_name, arguments, tool_use_id);
    }
    if context
        .bridge
        .subagent_reuse
        .note_inflight_launch(session_id, arguments, tool_use_id)
    {
        return DuplicateSkip::Forward;
    }
    tracing::info!(
        session_id,
        tool = original_name,
        tool_use_id,
        "skipped provider SubAgent launch because admission claim was taken"
    );
    DuplicateSkip::Rejected
}

fn occupied_duplicate_skip(
    context: ExternalToolContext<'_>,
    session_id: &str,
    original_name: &str,
    arguments: &mut Value,
    tool_use_id: &str,
) -> DuplicateSkip {
    if context
        .bridge
        .subagent_reuse
        .rewrite_launch_input(session_id, arguments)
        .is_some()
    {
        return DuplicateSkip::Forward;
    }
    if context
        .bridge
        .subagent_reuse
        .queue_inflight_follow_up(session_id, arguments)
    {
        tracing::info!(
            session_id,
            tool = original_name,
            tool_use_id,
            "queued follow-up for an inflight same-scope provider SubAgent"
        );
        return DuplicateSkip::Queued;
    }
    tracing::info!(
        session_id,
        tool = original_name,
        tool_use_id,
        "skipped duplicate same-scope same-model provider SubAgent launch"
    );
    DuplicateSkip::Rejected
}

async fn skip_or_queue_duplicate_launch(
    builder: &mut SegmentBuilder,
    context: ExternalToolContext<'_>,
    original_name: &str,
    arguments: &mut Value,
    tool_use_id: &str,
    request_id: Value,
) -> Result<bool> {
    match skip_duplicate_subagent_launch(context, original_name, arguments, tool_use_id) {
        DuplicateSkip::Forward => Ok(false),
        DuplicateSkip::Queued => {
            ack_queued_subagent_follow_up(builder, context, original_name, request_id).await?;
            Ok(true)
        }
        DuplicateSkip::Rejected => {
            builder
                .reject_duplicate_subagent(context, original_name, request_id)
                .await?;
            Ok(true)
        }
    }
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
        let follow_up = crate::anthropic::subagent_reuse::is_send_message_follow_up(&arguments);
        if reject_non_follow_up_launch(
            self,
            context,
            original_name,
            &mut arguments,
            reject_request_id.clone(),
            follow_up,
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
        if !follow_up
            && skip_or_queue_duplicate_launch(
                self,
                context,
                original_name,
                &mut arguments,
                &tool_use_id,
                request_id.clone(),
            )
            .await?
        {
            return Ok(());
        }
        let follow_up = crate::anthropic::subagent_reuse::is_send_message_follow_up(&arguments);
        if follow_up
            && !crate::anthropic::subagent_reuse::has_listed_send_message(
                context.session.external_tool_names.values(),
            )
        {
            return reject_unrequested_tool(
                context.bridge,
                context.session,
                ToolCall {
                    call_id,
                    name: "SendMessage".to_owned(),
                    arguments,
                    request_id,
                },
            )
            .await;
        }
        let emit_name = if follow_up {
            "SendMessage"
        } else {
            original_name
        };
        self.emit_external_tool_use(
            context,
            emit_name,
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
