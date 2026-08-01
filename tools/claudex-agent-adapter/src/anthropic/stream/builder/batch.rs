use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{SegmentBuilder, ToolCall, ensure_background_batch_launch};
use crate::anthropic::stream::builder::external_tool::ExternalToolContext;

pub(super) async fn dispatch(
    builder: &mut SegmentBuilder,
    context: ExternalToolContext<'_>,
    original_name: &str,
    call: ToolCall<'_>,
) -> Result<()> {
    let tasks = call
        .arguments
        .get("tasks")
        .and_then(Value::as_array)
        .context("batch Agent tasks missing")?;
    validate_batch_size(tasks.len())?;
    for (index, arguments) in tasks.iter().enumerate() {
        dispatch_task(
            builder,
            context,
            original_name,
            &call,
            tasks.len(),
            index,
            arguments,
        )
        .await?;
    }
    Ok(())
}

fn validate_batch_size(task_count: usize) -> Result<()> {
    let minimum = crate::anthropic::agent_batch::minimum_batch_size();
    let maximum = crate::anthropic::agent_batch::maximum_batch_size();
    if !(minimum..=maximum).contains(&task_count) {
        bail!("batch Agent tasks must contain between {minimum} and {maximum} launches");
    }
    Ok(())
}

async fn dispatch_task(
    builder: &mut SegmentBuilder,
    context: ExternalToolContext<'_>,
    original_name: &str,
    call: &ToolCall<'_>,
    task_count: usize,
    index: usize,
    arguments: &Value,
) -> Result<()> {
    let mut nested_arguments = arguments.clone();
    ensure_background_batch_launch(&mut nested_arguments);
    let nested = ToolCall {
        call_id: call.call_id,
        name: call.name,
        arguments: &nested_arguments,
        request_id: crate::anthropic::agent_batch::pending_marker(
            call.request_id.clone(),
            index,
            task_count,
        ),
    };
    builder
        .external_tool_call(context, original_name, nested)
        .await
}
