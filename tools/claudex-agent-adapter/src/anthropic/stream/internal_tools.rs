use std::sync::Arc;

use serde_json::{Value, json};

use super::super::{
    Bridge, Session,
    subscription::{SubscriptionOptions, run_subscription_model, subscription_prompt},
};
use super::ToolCall;

impl Bridge {
    pub(super) async fn spawn_internal_tool(
        &self,
        session: &Session,
        current_messages: &[Value],
        call: &ToolCall<'_>,
        model: &str,
    ) {
        let transcript = session.transcript.lock().await;
        let context = transcript
            .iter()
            .chain(current_messages)
            .cloned()
            .collect::<Vec<_>>();
        drop(transcript);
        let prompt = subscription_prompt(call.name, call.arguments, &context);
        let app = Arc::clone(&self.app);
        let model = model.to_owned();
        let program = self.subscription_program.clone();
        let subscription_slots = Arc::clone(&self.subscription_slots);
        let subscription_timeout = self.subscription_timeout;
        let request_id = call.request_id.clone();
        let parent_model = session.model.clone();
        tokio::spawn(async move {
            let options = SubscriptionOptions::internal(subscription_slots, subscription_timeout);
            let result = run_subscription_model(&program, &model, &prompt, options).await;
            let (text, success) = match result {
                Ok(text) => (text, true),
                Err(error) => (format!("Claude subscription call failed: {error:#}"), false),
            };
            let response = json!({
                "contentItems":[{"type":"inputText","text":text}],
                "success":success
            });
            if let Err(error) = app
                .respond_for_model(&parent_model, request_id, response)
                .await
            {
                tracing::error!(%error, "failed to return internal Claude tool result");
            }
        });
    }
}
