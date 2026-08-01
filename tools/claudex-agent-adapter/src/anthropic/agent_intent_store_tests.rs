#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn stored(id: &str, age: u64) -> StoredAgentIntent {
        StoredAgentIntent {
            client_user_id: Some("session".to_owned()),
            effort: Some("high".to_owned()),
            model_override: Some("grok-4.5".to_owned()),
            model_is_inherited: false,
            run_in_background: false,
            tool_use_id: id.to_owned(),
            created_unix_seconds: unix_seconds().saturating_sub(age),
        }
    }

    fn write_document(path: &Path, intents: Vec<StoredAgentIntent>) {
        fs::write(
            path,
            serde_json::to_vec(&StoredAgentIntents {
                version: CACHE_VERSION,
                intents,
            })
            .expect("serialize intents"),
        )
        .expect("write intents");
    }

    #[test]
    fn bounds_and_validates_loaded_intents() {
        let root = tempfile::tempdir().expect("store directory");
        let path = root.path().join("intents.json");
        let mut intents = (0..=super::super::agent_effort::MAX_PENDING_INTENTS)
            .map(|index| stored(&format!("tool-{index}"), 0))
            .collect::<Vec<_>>();
        intents.push(stored("", 0));
        intents.push(stored("stale", MAX_AGE_SECONDS + 1));
        write_document(&path, intents);
        let loaded = AgentIntentStore::at(path).load();
        assert_eq!(
            loaded.len(),
            super::super::agent_effort::MAX_PENDING_INTENTS
        );
        assert_eq!(
            loaded.front().expect("bounded intent").tool_use_id,
            "tool-1"
        );
    }

    #[test]
    fn load_discards_invalid_optional_values_and_accepts_absent_ones() {
        let root = tempfile::tempdir().expect("store directory");
        let path = root.path().join("intents.json");
        let mut invalid_effort = stored("invalid-effort", 0);
        invalid_effort.effort = Some("fast".to_owned());
        let mut invalid_model = stored("invalid-model", 0);
        invalid_model.model_override = Some(String::new());
        let mut optional_values = stored("optional-values", 0);
        optional_values.effort = None;
        optional_values.model_override = None;
        write_document(
            &path,
            vec![invalid_effort, invalid_model, optional_values],
        );
        let loaded = AgentIntentStore::at(path).load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.front().expect("valid intent").tool_use_id, "optional-values");
    }

    #[test]
    fn missing_corrupt_and_incompatible_cache_documents_are_ignored() {
        let root = tempfile::tempdir().expect("store directory");
        let path = root.path().join("intents.json");
        assert!(AgentIntentStore::at(path.clone()).load().is_empty());
        fs::write(&path, b"not-json").expect("corrupt cache");
        assert!(AgentIntentStore::at(path.clone()).load().is_empty());
        fs::write(
            &path,
            serde_json::to_vec(&StoredAgentIntents {
                version: CACHE_VERSION + 1,
                intents: vec![stored("tool", 0)],
            })
            .expect("serialize version"),
        )
        .expect("write version");
        assert!(AgentIntentStore::at(path).load().is_empty());
    }

    #[test]
    fn unreadable_cache_paths_and_current_user_stores_are_safe() {
        let root = tempfile::tempdir().expect("store directory");
        let directory_path = root.path().join("intents-directory");
        fs::create_dir(&directory_path).expect("cache directory");
        assert!(AgentIntentStore::at(directory_path).load().is_empty());
        assert!(AgentIntentStore::for_current_user().is_some());
    }

    #[test]
    fn saves_private_bounded_cache_and_cleans_failed_renames() {
        let root = tempfile::tempdir().expect("store directory");
        let path = root.path().join("nested/intents.json");
        let store = AgentIntentStore::at(path.clone());
        let intents = (0..=super::super::agent_effort::MAX_PENDING_INTENTS)
            .map(|index| stored(&format!("tool-{index}"), 0))
            .collect();
        store.save(intents);
        let loaded = store.load();
        assert_eq!(loaded.len(), super::super::agent_effort::MAX_PENDING_INTENTS);
        assert_eq!(loaded.front().expect("bounded intent").tool_use_id, "tool-1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path)
                    .expect("cache metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().expect("cache parent"))
                    .expect("parent metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }

        let failed = root.path().join("failed/intents.json");
        fs::create_dir_all(failed.parent().expect("failed parent")).expect("failed directory");
        fs::create_dir(&failed).expect("directory target");
        let failed_store = AgentIntentStore::at(failed);
        failed_store.save(vec![stored("tool", 0)]);
        assert!(!failed_store.temporary_path().exists());
    }

    #[test]
    fn persistence_snapshot_only_keeps_correlated_intents() {
        let now = unix_seconds();
        let pending = VecDeque::from([
            AgentEffortIntent {
                client_user_id: Some("session".to_owned()),
                prompt: "ordinary".to_owned(),
                correlated: false,
                effort: Some("low".to_owned()),
                model_override: None,
                model_is_inherited: false,
                run_in_background: false,
                tool_use_id: "ordinary".to_owned(),
                created_at: std::time::Instant::now(),
                created_unix_seconds: now,
            },
            AgentEffortIntent {
                client_user_id: None,
                prompt: String::new(),
                correlated: true,
                effort: Some("max".to_owned()),
                model_override: Some("gpt-5.6-luna".to_owned()),
                model_is_inherited: false,
                run_in_background: true,
                tool_use_id: "correlated".to_owned(),
                created_at: std::time::Instant::now(),
                created_unix_seconds: now,
            },
        ]);
        let snapshot = persistence_snapshot(&pending);
        assert_eq!(snapshot.len(), 1);
        let stored = snapshot.first().expect("correlated snapshot");
        assert_eq!(stored.tool_use_id, "correlated");
        assert_eq!(stored.effort.as_deref(), Some("max"));
        assert_eq!(stored.model_override.as_deref(), Some("gpt-5.6-luna"));
    }

    #[test]
    fn persistent_intents_restore_and_persist_their_store_snapshot() {
        let root = tempfile::tempdir().expect("store directory");
        let path = root.path().join("intents.json");
        let intents = AgentEffortIntents::with_store(path.clone());
        intents.persist(vec![stored("persisted", 0)]);
        let restored = AgentEffortIntents::with_store(path);
        let pending = restored.pending.lock().expect("pending intents");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.front().expect("restored intent").tool_use_id, "persisted");
        assert!(pending.front().expect("restored intent").correlated);
    }

    #[test]
    fn terminal_task_notifications_retire_only_the_correlated_session_intent() {
        let intents = AgentEffortIntents::default();
        let now = unix_seconds();
        intents
            .pending
            .lock()
            .expect("pending intents")
            .extend(["session-a", "session-b"].map(|client_user_id| AgentEffortIntent {
                client_user_id: Some(client_user_id.to_owned()),
                prompt: String::new(),
                correlated: true,
                effort: Some("high".to_owned()),
                model_override: Some("worker".to_owned()),
                model_is_inherited: false,
                run_in_background: true,
                tool_use_id: "tool-shared".to_owned(),
                created_at: std::time::Instant::now(),
                created_unix_seconds: now,
            }));
        let request = super::super::MessagesRequest {
            model: "main".to_owned(),
            system: serde_json::Value::Null,
            messages: vec![serde_json::json!({
                "role":"user",
                "content":"<task-notification>\n<task-id>task</task-id>\n<tool-use-id>tool-shared</tool-use-id>\n<status>completed</status>\n<result>done</result>\n</task-notification>"
            })],
            tools: Vec::new(),
            stream: false,
            output_config: serde_json::Value::Null,
            metadata: serde_json::json!({"user_id":"session-a"}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };

        intents.retire_terminal_task_notifications(&request);

        let pending = intents.pending.lock().expect("pending intents");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.front().unwrap().client_user_id.as_deref(), Some("session-b"));
    }

    #[test]
    fn default_intents_ignore_persistence_and_parent_defaults_to_current_directory() {
        AgentEffortIntents::default().persist(vec![stored("ignored", 0)]);
        assert_eq!(parent_directory(Path::new("intents.json")), Path::new("."));
    }
}
