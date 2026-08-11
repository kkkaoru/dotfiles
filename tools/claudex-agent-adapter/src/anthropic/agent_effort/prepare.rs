use serde_json::Value;

use super::{
    ADAPTER_EFFORT, ADAPTER_MODEL, IMPLICIT_MODEL, agent_prompt, background_launch, is_agent_tool,
    names::active_user_supplied_name, requested_model,
};
use crate::anthropic::agent_routing;

#[cfg(test)]
pub(in crate::anthropic) fn prepare_arguments(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
) -> (Option<Value>, Value) {
    prepare_arguments_for_user(
        tool_name,
        tool_use_id,
        arguments,
        &[],
        &serde_json::json!(null),
    )
}

pub(in crate::anthropic) fn prepare_arguments_for_user(
    tool_name: &str,
    tool_use_id: &str,
    arguments: &Value,
    user_messages: &[Value],
    _system: &Value,
) -> (Option<Value>, Value) {
    let mut correlated = arguments.clone();
    let Some(prompt) = agent_prompt(tool_name, arguments) else {
        let mut public = correlated.clone();
        return (
            None,
            sanitize_public_tool_arguments(tool_name, &mut public, user_messages),
        );
    };
    agent_routing::hydrate_routing_fields(&mut correlated);
    correlated["prompt"] = Value::String(super::super::agent_effort_matching::correlated_prompt(
        prompt,
        tool_use_id,
        requested_model(arguments),
    ));
    let mut public_arguments = correlated.clone();
    let mut claude_arguments =
        sanitize_public_tool_arguments(tool_name, &mut public_arguments, user_messages);
    let public = claude_arguments
        .as_object_mut()
        .expect("Agent arguments must be an object");
    public.remove("model");
    if public
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| !active_user_supplied_name(user_messages, name))
    {
        public.remove("name");
    }
    (Some(correlated), claude_arguments)
}

fn sanitize_public_tool_arguments(
    tool_name: &str,
    arguments: &mut Value,
    user_messages: &[Value],
) -> Value {
    let Some(public) = arguments.as_object_mut() else {
        return arguments.clone();
    };
    public.remove(ADAPTER_EFFORT);
    public.remove(ADAPTER_MODEL);
    public.remove(IMPLICIT_MODEL);
    if is_agent_tool(tool_name) {
        public.remove("model");
        public.insert(
            "run_in_background".to_owned(),
            Value::Bool(background_launch::agent_launch_is_background(
                tool_name,
                user_messages,
            )),
        );
    }
    arguments.clone()
}
