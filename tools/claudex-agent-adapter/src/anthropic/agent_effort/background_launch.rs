use std::collections::HashSet;

use serde_json::Value;

use super::{AgentEffortIntents, is_agent_tool, remove_expired};

#[path = "background_launch_text.rs"]
mod text;
use text::active_user_text;
pub(in crate::anthropic) use text::is_hook_or_mailbox_only;

const SYNC_NEEDLES: &[&str] = &[
    "synchronously",
    "synchronous result",
    "run in foreground",
    "don't background",
    "do not background",
    "wait for the result",
    "wait for results",
    "同期で",
    "同期して",
    "同期完了",
    "フォアグラウンド",
    "結果を待って",
    "待ってから",
    "終わるまで待って",
];

pub(in crate::anthropic) fn user_requires_synchronous_results(messages: &[Value]) -> bool {
    active_user_text(messages).is_some_and(|text| {
        let haystack = format!("{}\n{}", text, text.to_ascii_lowercase());
        SYNC_NEEDLES.iter().any(|needle| haystack.contains(needle))
    })
}

pub(in crate::anthropic) fn agent_launch_is_background(
    tool_name: &str,
    user_messages: &[Value],
) -> bool {
    is_agent_tool(tool_name) && !user_requires_synchronous_results(user_messages)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::anthropic) struct BackgroundLaunchIntent {
    pub(in crate::anthropic) model: Option<String>,
}

impl AgentEffortIntents {
    pub(in crate::anthropic) fn background_launches(
        &self,
        tool_use_ids: &[String],
    ) -> Option<Vec<BackgroundLaunchIntent>> {
        if tool_use_ids.is_empty() {
            return None;
        }
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        let mut seen = HashSet::with_capacity(tool_use_ids.len());
        let mut launches = Vec::with_capacity(tool_use_ids.len());
        for tool_use_id in tool_use_ids {
            launches.push(background_launch_intent(&pending, &mut seen, tool_use_id)?);
        }
        Some(launches)
    }
}

fn background_launch_intent<'a>(
    pending: &std::collections::VecDeque<super::AgentEffortIntent>,
    seen: &mut HashSet<&'a str>,
    tool_use_id: &'a String,
) -> Option<BackgroundLaunchIntent> {
    if !seen.insert(tool_use_id.as_str()) {
        return None;
    }
    let intent = pending
        .iter()
        .find(|intent| intent.tool_use_id == *tool_use_id)?;
    intent.run_in_background.then(|| BackgroundLaunchIntent {
        model: intent.model_override.clone(),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "background_launch_tests.rs"]
mod tests;
