use std::collections::HashSet;

use super::{AgentEffortIntents, remove_expired};

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
        let user_messages = [json!({
            "role":"user",
            "content":"delegate synthetic work with worker-model"
        })];
        for (id, background) in [("background", true), ("foreground", false)] {
            intents.record_from_user_messages(
                AgentEffortRecord {
                    client_user_id: Some("main"),
                    tool_name: "Agent",
                    tool_use_id: id.to_owned(),
                    parent_model: "main-model",
                    arguments: &json!({
                        "prompt":format!("synthetic {id}"),
                        "claudex_model":"worker-model",
                        "run_in_background":background
                    }),
                    user_messages: &user_messages,
                    system: &Value::Null,
                },
                None,
            );
        }

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
}
