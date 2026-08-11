use serde_json::Value;

use super::{
    AgentEffortIntents, MessagesRequest, intents_helpers, persistence_snapshot, remove_expired,
    terminal_task_notification_ids,
};

impl AgentEffortIntents {
    pub(in crate::anthropic) fn remove_tool_results<'a>(
        &self,
        tool_use_ids: impl Iterator<Item = &'a str>,
    ) {
        let ids = tool_use_ids.collect::<Vec<_>>();
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        pending.retain(|intent| intent.correlated || !ids.contains(&intent.tool_use_id.as_str()));
        let snapshot = persistence_snapshot(&pending);
        drop(pending);
        self.persist(snapshot);
    }

    pub(in crate::anthropic) fn retire_terminal_task_notifications(
        &self,
        request: &MessagesRequest,
    ) {
        let ids = terminal_task_notification_ids(&request.messages);
        if ids.is_empty() {
            return;
        }
        let client_user_id = request.metadata.get("user_id").and_then(Value::as_str);
        let mut pending = self.pending.lock().expect("agent effort intents poisoned");
        remove_expired(&mut pending);
        pending.retain(|intent| {
            intents_helpers::retain_terminal_intent(intent, &ids, client_user_id)
        });
        let snapshot = persistence_snapshot(&pending);
        drop(pending);
        self.persist(snapshot);
    }
}
