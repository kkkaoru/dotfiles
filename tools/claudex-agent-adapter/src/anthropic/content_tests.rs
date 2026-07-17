#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::Arc,
        time::Instant,
    };

    use serde_json::{Value, json};
    use tokio::sync::{Mutex, Semaphore};

    use super::{
        ToolResult, content_text, full_transcript_input, matching_transcript_len,
        take_pending_results,
    };
    use crate::anthropic::Session;

    #[tokio::test]
    async fn accepts_pending_and_already_consumed_results() {
        let session = session(
            [("pending".to_owned(), json!("call"))].into(),
            ["consumed".to_owned()].into(),
            Vec::new(),
        )
        .await;
        let results = vec![result("pending"), result("consumed")];
        let responses = take_pending_results(&session, results)
            .await
            .expect("valid results");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].0, "call");
        assert!(session.pending_since.lock().expect("clock").is_none());
    }

    #[tokio::test]
    async fn rejects_duplicate_unknown_and_mismatched_transcripts() {
        let session = session(
            [
                ("one".to_owned(), json!("first")),
                ("two".to_owned(), json!("second")),
            ]
            .into(),
            HashSet::new(),
            vec![json!({"role":"user","content":"original"})],
        )
        .await;
        assert!(
            take_pending_results(&session, vec![result("one"), result("one")])
                .await
                .is_err()
        );
        assert!(
            take_pending_results(&session, vec![result("unknown")])
                .await
                .is_err()
        );
        let responses = take_pending_results(&session, vec![result("one")])
            .await
            .expect("one pending result");
        assert_eq!(responses.len(), 1);
        assert!(session.pending_since.lock().expect("clock").is_some());
        assert!(
            matching_transcript_len(&session, &[json!({"role":"user","content":"different"})])
                .await
                .is_none()
        );
        assert!(matching_transcript_len(&session, &[]).await.is_none());
    }

    #[test]
    fn covers_text_and_transcript_short_circuit_inputs() {
        assert_eq!(content_text(&json!(null)), "");
        assert_eq!(
            content_text(&json!([
                {"type":"image","text":"ignored"},
                {"type":"text"},
                {"type":"text","text":"kept"}
            ])),
            "kept"
        );
        let assistant = json!({"role":"assistant","content":"reply"});
        let input = full_transcript_input(&[assistant]);
        assert!(
            input[0]["text"]
                .as_str()
                .expect("transcript")
                .contains("role-tagged history")
        );
    }

    fn result(tool_use_id: &str) -> ToolResult {
        ToolResult {
            tool_use_id: tool_use_id.to_owned(),
            content_items: Vec::new(),
            is_error: false,
        }
    }

    async fn session(
        pending_tools: HashMap<String, Value>,
        consumed_tool_ids: HashSet<String>,
        transcript: Vec<Value>,
    ) -> Session {
        let semaphore = Arc::new(Semaphore::new(1));
        Session {
            thread_id: "thread".to_owned(),
            signature: "signature".to_owned(),
            transcript: Mutex::new(transcript),
            pending_tools: Mutex::new(pending_tools),
            consumed_tool_ids: Mutex::new(consumed_tool_ids),
            internal_tools: HashMap::new(),
            external_tool_names: HashMap::new(),
            client_user_id: None,
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(Some(Instant::now())),
            _slot: semaphore.acquire_owned().await.expect("session slot"),
        }
    }
}
