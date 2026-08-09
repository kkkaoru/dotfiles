#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use serde_json::json;

    fn names() -> HashMap<String, String> {
        HashMap::from([
            ("cc_Agent_0".to_owned(), "Agent".to_owned()),
            ("cc_Bash_1".to_owned(), "Bash".to_owned()),
            ("cc_Task_2".to_owned(), "Task".to_owned()),
        ])
    }

    #[test]
    fn bridges_only_agent_and_task_when_request_supplied() {
        assert!(is_client_executed_bridge_tool("Agent"));
        assert!(is_client_executed_bridge_tool("Task"));
        assert!(!is_client_executed_bridge_tool("Bash"));
        let map = names();
        let agent = json!({
            "params":{
                "callId":"c1",
                "tool":"Agent",
                "status":"pending",
                "arguments":{"prompt":"do work"}
            }
        });
        let bridged = bridge_provider_tool_call(&map, &agent).expect("Agent bridges");
        assert_eq!(bridged.call_id, "c1");
        assert_eq!(bridged.name, "Agent");
        assert!(is_acp_bridge_request_id(&bridged.request_id));
        assert_eq!(bridged.arguments["prompt"], "do work");
        assert_eq!(bridged.arguments["description"], "do work");
        let task = json!({
            "params":{
                "callId":"c2",
                "tool":"cc_Task_2",
                "status":"in_progress",
                "arguments":{"prompt":"explore"}
            }
        });
        assert_eq!(
            bridge_provider_tool_call(&map, &task)
                .expect("Task bridges")
                .name,
            "Task"
        );
    }

    #[test]
    fn bridges_spawn_subagent_onto_agent_when_claude_supplied_agent() {
        let map = names();
        let spawn = json!({
            "params":{
                "callId":"s1",
                "tool":"spawn_subagent",
                "status":"pending",
                "arguments":{
                    "description":"smoke",
                    "prompt":"CHILD_OK",
                    "subagent_type":"grok-native-high-plugin-v3:claudex-high",
                    "run_in_background":false
                }
            }
        });
        let bridged = bridge_provider_tool_call(&map, &spawn).expect("spawn bridges");
        assert_eq!(bridged.name, "Agent");
        assert_eq!(bridged.arguments["subagent_type"], "claudex-grok");
        assert_eq!(bridged.arguments["run_in_background"], true);
        assert_eq!(bridged.arguments["description"], "smoke");
    }

    #[test]
    fn never_bridges_native_tools_or_failed_calls() {
        let map = names();
        let bash = json!({
            "params":{
                "callId":"b1",
                "tool":"Bash",
                "status":"pending",
                "arguments":{"command":"ls"}
            }
        });
        assert!(bridge_provider_tool_call(&map, &bash).is_none());
        let failed = json!({
            "params":{
                "callId":"c3",
                "tool":"Agent",
                "status":"failed",
                "arguments":{"prompt":"late"}
            }
        });
        assert!(bridge_provider_tool_call(&map, &failed).is_none());
        let completed = json!({
            "params":{
                "callId":"c3b",
                "tool":"Agent",
                "status":"completed",
                "arguments":{"prompt":"late"}
            }
        });
        let bridged =
            bridge_provider_tool_call(&map, &completed).expect("completed launch bridges");
        assert_eq!(bridged.name, "Agent");
        assert!(
            bridge_provider_tool_call(
                &HashMap::new(),
                &json!({
                    "params":{"callId":"c4","tool":"Agent","status":"pending","arguments":{}}
                })
            )
            .is_none()
        );
    }

    #[test]
    fn matches_title_when_tool_label_is_generic() {
        let map = names();
        let event = json!({
            "params":{
                "callId":"t1",
                "tool":"Tool",
                "title":"Agent",
                "status":"pending",
                "arguments":{"prompt":"via title"}
            }
        });
        let bridged = bridge_provider_tool_call(&map, &event).expect("title Agent bridges");
        assert_eq!(bridged.name, "Agent");
    }

    #[test]
    fn does_not_bridge_incomplete_cursor_native_task() {
        let map = names();
        // Observed with Cursor `auto` as the outer/main model: native Task opens with
        // meta fields only, which previously became Agent tool_use missing prompt.
        let incomplete = json!({
            "params":{
                "callId":"cursor-task-1",
                "tool":"Task",
                "title":"Task",
                "status":"pending",
                "arguments":{"_toolName":"task","run_in_background":true}
            }
        });
        assert!(bridge_provider_tool_call(&map, &incomplete).is_none());
        assert!(is_unbridged_launch_progress(&map, &incomplete));

        let empty = json!({
            "params":{
                "callId":"cursor-task-2",
                "tool":"task",
                "status":"in_progress",
                "arguments":{}
            }
        });
        assert!(bridge_provider_tool_call(&map, &empty).is_none());
        assert!(is_unbridged_launch_progress(&map, &empty));

        let completed = json!({
            "params":{
                "callId":"cursor-task-3",
                "tool":"Task",
                "title":"Task",
                "status":"completed",
                "arguments":{"prompt":"late"}
            }
        });
        assert!(bridge_provider_tool_call(&map, &completed).is_some());
        assert!(!is_unbridged_launch_progress(&map, &completed));

        let bash = json!({
            "params":{
                "callId":"bash-1",
                "tool":"Bash",
                "status":"pending",
                "arguments":{"command":"ls"}
            }
        });
        assert!(!is_unbridged_launch_progress(&map, &bash));
    }

    #[test]
    fn bash_titles_with_task_or_agent_substrings_are_not_unbridged_launches() {
        let map = names();
        let schtasks = json!({
            "params":{
                "callId":"bash-schtasks",
                "tool":"Bash",
                "title":"`cd /repo && prlctl exec 'Windows 11' cmd.exe /c schtasks /Query /TN \"PC-KEIBA Auto Update\"`",
                "status":"in_progress",
                "arguments":{"command":"prlctl exec Windows schtasks /Query"}
            }
        });
        assert!(bridge_provider_tool_call(&map, &schtasks).is_none());
        assert!(!is_unbridged_launch_progress(&map, &schtasks));

        let history = json!({
            "params":{
                "callId":"bash-ctx",
                "tool":"Bash",
                "title":"Shell: ctx search (Search local agent history for RESCORE_ENABLED)",
                "status":"pending",
                "arguments":{
                    "command":"ctx search RESCORE_ENABLED",
                    "description":"Search local agent history",
                    "timeout":60_000
                }
            }
        });
        assert!(bridge_provider_tool_call(&map, &history).is_none());
        assert!(!is_unbridged_launch_progress(&map, &history));
    }

    #[test]
    fn bridges_cursor_mcp_title_with_launch_arguments() {
        let map = names();
        let event = json!({
            "params":{
                "callId":"mcp-1",
                "tool":"MCP",
                "title":"MCP",
                "status":"completed",
                "arguments":{
                    "description":"auto-subagent-smoke4",
                    "prompt":"Reply with exactly AGENT_PONG4 then stop.",
                    "subagent_type":"claudex-ollama-glm-5-2",
                    "claudex_model":"glm-5.2:cloud",
                    "run_in_background":true
                }
            }
        });
        let bridged = bridge_provider_tool_call(&map, &event).expect("MCP launch bridges");
        assert_eq!(bridged.name, "Agent");
        assert_eq!(
            bridged.arguments["prompt"],
            "Reply with exactly AGENT_PONG4 then stop."
        );
        assert_eq!(bridged.arguments["subagent_type"], "claudex-ollama-glm-5-2");
        assert!(!is_unbridged_launch_progress(&map, &event));

        let incomplete = json!({
            "params":{
                "callId":"mcp-2",
                "tool":"MCP",
                "title":"MCP",
                "status":"pending",
                "arguments":{"run_in_background":true}
            }
        });
        assert!(bridge_provider_tool_call(&map, &incomplete).is_none());
        assert!(is_unbridged_launch_progress(&map, &incomplete));
    }

    #[test]
    fn bridges_when_prompt_arrives_via_alias_after_normalize() {
        let map = names();
        let event = json!({
            "params":{
                "callId":"alias-1",
                "tool":"Task",
                "status":"pending",
                "arguments":{
                    "_toolName":"task",
                    "title":"gap fix",
                    "task":"Investigate the wasm build failure",
                    "run_in_background":true
                }
            }
        });
        let bridged = bridge_provider_tool_call(&map, &event).expect("alias prompt bridges");
        assert_eq!(bridged.name, "Task");
        assert_eq!(
            bridged.arguments["prompt"],
            "Investigate the wasm build failure"
        );
        assert_eq!(bridged.arguments["description"], "gap fix");
        assert!(bridged.arguments.get("_toolName").is_none());
        assert!(!is_unbridged_launch_progress(&map, &event));
    }

    #[test]
    fn covers_has_agent_tool_all_branches() {
        let empty = HashMap::new();
        assert!(!has_agent_tool(&empty));

        let by_value = HashMap::from([("key".to_owned(), "Agent".to_owned())]);
        assert!(has_agent_tool(&by_value));

        let by_key = HashMap::from([("Agent".to_owned(), "SomeOther".to_owned())]);
        assert!(has_agent_tool(&by_key));

        let ends_with = HashMap::from([("MyAgent".to_owned(), "Tool".to_owned())]);
        assert!(has_agent_tool(&ends_with));

        let contains_in_key = HashMap::from([("AgentConfig".to_owned(), "X".to_owned())]);
        assert!(has_agent_tool(&contains_in_key));
    }

    #[test]
    fn covers_looks_like_launch_tool_variants() {
        assert!(looks_like_launch_tool("agent"));
        assert!(looks_like_launch_tool("AGENT"));
        assert!(looks_like_launch_tool("Task"));
        assert!(looks_like_launch_tool("spawn_subagent"));
        assert!(looks_like_launch_tool("mcp"));
        assert!(looks_like_launch_tool("MCP"));
        assert!(looks_like_launch_tool("claudex-launch-foo"));
        assert!(looks_like_launch_tool("mcp__claudex-foo"));
        assert!(looks_like_launch_tool("mcp__agent"));
        assert!(looks_like_launch_tool("mcp__task"));
        assert!(looks_like_launch_tool("foo__agent"));
        assert!(looks_like_launch_tool("bar__task"));
        assert!(looks_like_launch_tool("prefix_spawn_subagent"));
        assert!(!looks_like_launch_tool("Bash"));
        assert!(!looks_like_launch_tool("Read"));
    }

    #[test]
    fn covers_looks_like_launch_arguments_false_paths() {
        assert!(!looks_like_launch_arguments(&json!(null)));
        assert!(!looks_like_launch_arguments(&json!("string")));
        assert!(!looks_like_launch_arguments(&json!(42)));
        assert!(!looks_like_launch_arguments(&json!({"no_prompt": "value"})));
        assert!(!looks_like_launch_arguments(&json!({"prompt": ""})));
        assert!(!looks_like_launch_arguments(&json!({"prompt": "   "})));
    }

    #[test]
    fn covers_looks_like_launch_arguments_true_branches() {
        assert!(looks_like_launch_arguments(&json!({
            "prompt": "task",
            "subagent_type": "foo"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "prompt": "work",
            "run_in_background": true
        })));
        assert!(looks_like_launch_arguments(&json!({
            "prompt": "go",
            "claudex_model": "bar"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "prompt": "x",
            "claudex_effort": "high"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "prompt": "y",
            "_toolName": "task"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "instruction": "z",
            "description": "test"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "message": "m",
            "title": "t"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "query": "q",
            "name": "n"
        })));
        assert!(looks_like_launch_arguments(&json!({
            "input": "i",
            "summary": "s"
        })));
    }

    #[test]
    fn covers_launch_tool_name_from_arguments_branches() {
        let no_agent = HashMap::from([("key".to_owned(), "Bash".to_owned())]);
        assert!(launch_tool_name_from_arguments(&json!({"prompt": "x"}), &no_agent).is_none());

        let non_object = HashMap::from([("k".to_owned(), "Agent".to_owned())]);
        assert!(launch_tool_name_from_arguments(&json!("not-object"), &non_object).is_none());

        let with_agent = HashMap::from([
            ("cc_Agent".to_owned(), "Agent".to_owned()),
            ("cc_Task".to_owned(), "Task".to_owned()),
        ]);
        let task_toolname = launch_tool_name_from_arguments(
            &json!({"prompt": "x", "_toolName": "task"}),
            &with_agent,
        );
        assert_eq!(task_toolname, Some("Task".to_owned()));

        let task_ends_with = launch_tool_name_from_arguments(
            &json!({"prompt": "x", "_toolName": "foo__task"}),
            &with_agent,
        );
        assert_eq!(task_ends_with, Some("Task".to_owned()));

        let task_contains = launch_tool_name_from_arguments(
            &json!({"prompt": "x", "_toolName": "contains-task-here"}),
            &with_agent,
        );
        assert_eq!(task_contains, Some("Task".to_owned()));

        let no_task_available = HashMap::from([("cc_Agent".to_owned(), "Agent".to_owned())]);
        let fallback_agent = launch_tool_name_from_arguments(
            &json!({"prompt": "x", "_toolName": "task"}),
            &no_task_available,
        );
        assert_eq!(fallback_agent, Some("Agent".to_owned()));
    }

    #[test]
    fn covers_map_launch_name_requested_original() {
        let names = HashMap::from([
            ("provided_Agent".to_owned(), "Agent".to_owned()),
            ("provided_Task".to_owned(), "Task".to_owned()),
        ]);
        assert_eq!(
            map_launch_name("provided_Agent", &names),
            Some("Agent".to_owned())
        );
        assert_eq!(
            map_launch_name("provided_Task", &names),
            Some("Task".to_owned())
        );
    }

    #[test]
    fn covers_map_launch_name_launch_tool_candidates() {
        let names = HashMap::from([("cc_Agent".to_owned(), "Agent".to_owned())]);
        assert_eq!(map_launch_name("agent", &names), Some("Agent".to_owned()));
        assert_eq!(map_launch_name("task", &names), Some("Agent".to_owned()));
        assert_eq!(
            map_launch_name("spawn_subagent", &names),
            Some("Agent".to_owned())
        );
        assert_eq!(map_launch_name("mcp", &names), Some("Agent".to_owned()));

        let with_task = HashMap::from([
            ("cc_Agent".to_owned(), "Agent".to_owned()),
            ("cc_Task".to_owned(), "Task".to_owned()),
        ]);
        assert_eq!(map_launch_name("task", &with_task), Some("Task".to_owned()));
        assert_eq!(
            map_launch_name("mcp__task", &with_task),
            Some("Task".to_owned())
        );
    }

    #[test]
    fn covers_map_launch_name_none() {
        let names = HashMap::from([("cc_Bash".to_owned(), "Bash".to_owned())]);
        assert!(map_launch_name("unknown", &names).is_none());
        assert!(map_launch_name("Bash", &names).is_none());
    }

    #[test]
    fn covers_normalize_launch_arguments_non_object() {
        let result = normalize_launch_arguments("Agent", &json!("string"));
        assert_eq!(result["value"], "string");

        let num = normalize_launch_arguments("Agent", &json!(42));
        assert_eq!(num["value"], 42);
    }

    #[test]
    fn covers_normalize_launch_arguments_aliases() {
        let event = json!({
            "task": "solve problem",
            "instruction": "alt instruction"
        });
        let result = normalize_launch_arguments("Agent", &event);
        assert_eq!(result["prompt"], "solve problem");
        assert!(result.get("task").is_none());

        let desc_event = json!({
            "prompt": "work",
            "title": "my title",
            "name": "alt name"
        });
        let result = normalize_launch_arguments("Agent", &desc_event);
        assert_eq!(result["description"], "my title");
        assert!(result.get("title").is_none());
    }

    #[test]
    fn covers_normalize_launch_arguments_description_from_prompt() {
        let event = json!({
            "prompt": "This is a very long prompt that should be truncated to 60 chars for description"
        });
        let result = normalize_launch_arguments("Agent", &event);
        assert_eq!(
            result["description"],
            "This is a very long prompt that should be truncated to 60 ch"
        );
    }

    #[test]
    fn covers_normalize_launch_arguments_spawn_subagent() {
        let spawn = json!({
            "prompt": "go",
            "subagent_type": "grok-native-high-plugin-v3:claudex-high"
        });
        let result = normalize_launch_arguments("spawn_subagent", &spawn);
        assert_eq!(result["subagent_type"], "claudex-grok");
        assert_eq!(result["run_in_background"], true);

        let mcp_spawn = json!({"prompt": "x", "subagent_type": "other"});
        let result = normalize_launch_arguments("MCP__Spawn_Subagent", &mcp_spawn);
        assert_eq!(result["run_in_background"], true);

        let no_subagent = json!({"prompt": "y"});
        let result = normalize_launch_arguments("spawn_subagent", &no_subagent);
        assert_eq!(result["run_in_background"], true);
    }

    #[test]
    fn covers_normalize_launch_arguments_metadata_cleanup() {
        let event = json!({
            "prompt": "work",
            "_toolName": "task",
            "_tool_name": "alt",
            "other": "value"
        });
        let result = normalize_launch_arguments("Agent", &event);
        assert!(result.get("_toolName").is_none());
        assert!(result.get("_tool_name").is_none());
        assert_eq!(result["other"], "value");
    }

    #[test]
    fn covers_bridgeable_status_all_cases() {
        assert!(bridgeable_status(Some("pending")));
        assert!(bridgeable_status(Some("in_progress")));
        assert!(bridgeable_status(Some("started")));
        assert!(bridgeable_status(Some("completed")));
        assert!(bridgeable_status(None));
        assert!(!bridgeable_status(Some("failed")));
        assert!(!bridgeable_status(Some("cancelled")));
        assert!(bridgeable_status(Some("unknown")));
    }

    #[test]
    fn covers_is_compact_tool_label_edge_cases() {
        assert!(is_compact_tool_label("Agent"));
        assert!(!is_compact_tool_label(""));
        assert!(!is_compact_tool_label("   "));
        assert!(!is_compact_tool_label(&"a".repeat(65)));
        assert!(!is_compact_tool_label("line\nbreak"));
        assert!(!is_compact_tool_label("`quoted`"));
        assert!(!is_compact_tool_label("has spaces here"));
        assert!(is_compact_tool_label("agent"));
        assert!(is_compact_tool_label("mcp__agent"));
    }

    #[test]
    fn covers_looks_like_mcp_surface_variants() {
        assert!(looks_like_mcp_surface("mcp"));
        assert!(looks_like_mcp_surface("MCP"));
        assert!(looks_like_mcp_surface("mcp:tool"));
        assert!(looks_like_mcp_surface("mcp "));
        assert!(looks_like_mcp_surface("claudex-launch"));
        assert!(looks_like_mcp_surface("mcp+agent"));
        assert!(looks_like_mcp_surface("mcp+task"));
        assert!(!looks_like_mcp_surface("Bash"));
    }

    #[test]
    fn covers_is_unbridged_launch_progress_missing_params() {
        let map = names();
        assert!(!is_unbridged_launch_progress(&map, &json!({})));
        assert!(!is_unbridged_launch_progress(
            &map,
            &json!({"params": null})
        ));
    }

    #[test]
    fn covers_is_unbridged_launch_progress_true_cases() {
        let map = names();
        let unbridged = json!({
            "params": {
                "callId": "x",
                "tool": "Agent",
                "status": "pending",
                "arguments": {"_toolName": "task"}
            }
        });
        assert!(is_unbridged_launch_progress(&map, &unbridged));

        let mcp_incomplete = json!({
            "params": {
                "callId": "m",
                "tool": "MCP",
                "status": "pending",
                "arguments": {}
            }
        });
        assert!(is_unbridged_launch_progress(&map, &mcp_incomplete));
    }

    #[test]
    fn covers_bridge_provider_tool_call_with_mcp_hint_queue_path() {
        let map = names();
        let mcp_event = json!({
            "params": {
                "callId": "mcp-queue",
                "tool": "MCP",
                "status": "pending",
                "arguments": {"run_in_background": true}
            }
        });
        let result = bridge_provider_tool_call_with_mcp_hint(&map, &mcp_event, None);
        assert!(result.is_none());
    }

    #[test]
    fn covers_requested_original_name_fallback() {
        let names = HashMap::from([
            ("k1".to_owned(), "Agent".to_owned()),
            ("Agent".to_owned(), "DummyVal".to_owned()),
        ]);
        assert_eq!(requested_original_name(&names, "Agent"), Some("DummyVal"));

        let value_match = HashMap::from([("k".to_owned(), "MyName".to_owned())]);
        assert_eq!(
            requested_original_name(&value_match, "MyName"),
            Some("MyName")
        );

        let no_match = HashMap::from([("k".to_owned(), "Other".to_owned())]);
        assert!(requested_original_name(&no_match, "Unknown").is_none());
    }
}
