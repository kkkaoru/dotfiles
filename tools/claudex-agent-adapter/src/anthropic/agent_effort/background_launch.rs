use std::collections::HashSet;

use serde_json::Value;

use super::{AgentEffortIntents, is_agent_tool, remove_expired};

const SYNC_NEEDLES: &[&str] = &[
    "synchronously",
    "synchronous result",
    "run in foreground",
    "don't background",
    "do not background",
    "wait for the result",
    "wait for results",
    "同期で",
    "同期完了",
    "フォアグラウンド",
    "結果を待って",
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

fn active_user_text(messages: &[Value]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            return None;
        }
        let text = user_message_text(message)?;
        if text.contains("<agent-message")
            || text.contains("<teammate-message")
            || text
                .trim_start()
                .starts_with("Another Claude session sent a message")
        {
            return None;
        }
        Some(text)
    })
}

fn user_message_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
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
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::anthropic::AgentEffortRecord;

    #[test]
    fn requires_unique_known_background_agent_launches() {
        let intents = AgentEffortIntents::default();
        let background_messages = [json!({
            "role":"user",
            "content":"delegate synthetic work with worker-model"
        })];
        let sync_messages = [json!({
            "role":"user",
            "content":"同期で結果を待ってから次へ進めて"
        })];
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("main"),
                tool_name: "Agent",
                tool_use_id: "background".to_owned(),
                parent_model: "main-model",
                arguments: &json!({
                    "prompt":"synthetic background",
                    "claudex_model":"worker-model",
                    "run_in_background":false
                }),
                user_messages: &background_messages,
                system: &Value::Null,
            },
            None,
        );
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("main"),
                tool_name: "Agent",
                tool_use_id: "foreground".to_owned(),
                parent_model: "main-model",
                arguments: &json!({
                    "prompt":"synthetic foreground",
                    "claudex_model":"worker-model",
                    "run_in_background":false
                }),
                user_messages: &sync_messages,
                system: &Value::Null,
            },
            None,
        );

        let launches = intents
            .background_launches(&["background".to_owned()])
            .expect("known background launch");
        assert_eq!(launches.len(), 1);
        assert_eq!(launches[0].model.as_deref(), Some("worker-model"));
        assert!(
            intents
                .background_launches(&["foreground".to_owned()])
                .is_none()
        );
        assert!(
            intents
                .background_launches(&["unknown".to_owned()])
                .is_none()
        );
        assert!(
            intents
                .background_launches(&["background".to_owned(), "background".to_owned()])
                .is_none()
        );
        assert!(intents.background_launches(&[]).is_none());
    }

    #[test]
    fn cc_array_content_joins_reminder_and_user_text_for_sync_detection() {
        let reminder = json!({
            "type":"text",
            "text":"<system-reminder>\nClaudex routing\n</system-reminder>"
        });
        assert!(!user_requires_synchronous_results(&[json!({
            "role":"user",
            "content":[
                reminder.clone(),
                {"type":"text","text":"Investigate the neon pooler next."}
            ]
        })]));
        assert!(user_requires_synchronous_results(&[json!({
            "role":"user",
            "content":[
                reminder,
                {"type":"text","text":"同期で結果を待ってから次へ進めて"}
            ]
        })]));
    }
}
