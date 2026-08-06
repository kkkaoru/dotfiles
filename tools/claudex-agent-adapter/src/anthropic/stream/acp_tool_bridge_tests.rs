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
                    "subagent_type":"grok-native-high-plugin-v3:claudex-high"
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
        let bridged = bridge_provider_tool_call(&map, &completed).expect("completed launch bridges");
        assert_eq!(bridged.name, "Agent");
        assert!(bridge_provider_tool_call(&HashMap::new(), &json!({
            "params":{"callId":"c4","tool":"Agent","status":"pending","arguments":{}}
        })).is_none());
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
        assert_eq!(bridged.arguments["prompt"], "Reply with exactly AGENT_PONG4 then stop.");
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
        assert_eq!(bridged.arguments["prompt"], "Investigate the wasm build failure");
        assert_eq!(bridged.arguments["description"], "gap fix");
        assert!(bridged.arguments.get("_toolName").is_none());
        assert!(!is_unbridged_launch_progress(&map, &event));
    }
}
