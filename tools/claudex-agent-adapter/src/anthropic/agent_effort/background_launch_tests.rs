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
        fn with_reminder(text: &str) -> Value {
            json!({
                "role":"user",
                "content":[
                    {
                        "type":"text",
                        "text":"<system-reminder>\nClaudex routing\n</system-reminder>"
                    },
                    {"type":"text","text":text}
                ]
            })
        }
        assert!(!user_requires_synchronous_results(&[with_reminder(
            "Investigate the neon pooler next."
        )]));
        assert!(user_requires_synchronous_results(&[with_reminder(
            "同期で結果を待ってから次へ進めて"
        )]));
        assert!(user_requires_synchronous_results(&[with_reminder(
            "待ってから次へ進めて"
        )]));
        assert!(user_requires_synchronous_results(&[with_reminder(
            "同期して結果を見てから続けて"
        )]));
    }

    #[test]
    fn trailing_system_reminder_message_does_not_hide_sync_instruction() {
        let messages = vec![
            json!({
                "role":"user",
                "content":"結果を待ってから次へ進めて"
            }),
            json!({
                "role":"user",
                "content":"<system-reminder>\nPostToolUse hook noise\n</system-reminder>"
            }),
        ];
        assert!(
            user_requires_synchronous_results(&messages),
            "hook-only trailing user message must not force background launches"
        );
    }
