use serde_json::Value;

use super::{
    ADAPTER_EFFORT, ADAPTER_MODEL, IMPLICIT_MODEL, agent_prompt, background_launch, is_agent_tool,
    names::active_user_supplied_name, requested_model,
};
use crate::anthropic::agent_routing;

const ASSIGNED_WORKTREE_PROMPT_GUARD: &str = "Runtime worktree rule: the worktree and cwd assigned by Claude Code are authoritative. Do not switch to a preferred or existing worktree path named in the task. Do not call EnterWorktree or ExitWorktree, use git -C, or cd outside the assigned directory. If the requested target differs, report the conflict to the parent instead of changing directories.";
const AGENT_PUBLIC_KEYS: [&str; 7] = [
    "description",
    "prompt",
    "subagent_type",
    "name",
    "run_in_background",
    "isolation",
    "effort",
];
const SEND_MESSAGE_PUBLIC_KEYS: [&str; 3] = ["to", "summary", "message"];

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
    let prompt = super::super::agent_effort_matching::strip_correlation_suffix(prompt);
    let prompt = prompt_with_worktree_guard(prompt, arguments);
    correlated["prompt"] = Value::String(super::super::agent_effort_matching::correlated_prompt(
        &prompt,
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

fn prompt_with_worktree_guard(prompt: &str, arguments: &Value) -> String {
    if has_runtime_worktree_override(arguments) {
        format!("{prompt}\n\n{ASSIGNED_WORKTREE_PROMPT_GUARD}")
    } else {
        prompt.to_owned()
    }
}

fn has_runtime_worktree_override(arguments: &Value) -> bool {
    arguments
        .get("isolation")
        .and_then(Value::as_str)
        .is_some_and(|isolation| isolation == "worktree")
        || arguments
            .get("cwd")
            .and_then(Value::as_str)
            .is_some_and(|cwd| !cwd.trim().is_empty())
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
    // Claude Code Agent/Task dropped `resume` in v2.1.77. Extra keys with
    // additionalProperties:false surface as "Invalid tool parameters".
    public.remove("resume");
    public.remove("resume_from");
    if tool_name == "SendMessage" {
        public.retain(|key, _| SEND_MESSAGE_PUBLIC_KEYS.contains(&key.as_str()));
        return arguments.clone();
    }
    if is_agent_tool(tool_name) {
        public.remove("model");
        public.remove("cwd");
        public.remove("background");
        public.remove("capability_mode");
        public.insert(
            "run_in_background".to_owned(),
            Value::Bool(background_launch::agent_launch_is_background(
                tool_name,
                user_messages,
            )),
        );
        public.retain(|key, _| AGENT_PUBLIC_KEYS.contains(&key.as_str()));
    }
    if tool_name == "Bash" {
        promote_bash_command(public);
    }
    arguments.clone()
}

fn promote_bash_command(public: &mut serde_json::Map<String, Value>) {
    if public
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| !command.is_empty())
    {
        return;
    }
    let alias = ["cmd", "script", "bash"]
        .into_iter()
        .find_map(|key| match public.remove(key) {
            Some(Value::String(command)) if !command.is_empty() => Some(command),
            _ => None,
        });
    if let Some(command) = alias {
        public.insert("command".to_owned(), Value::String(command));
    }
}
