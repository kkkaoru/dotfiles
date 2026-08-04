use std::{
    fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    process::Command,
    thread,
    time::Duration,
};

use serde_json::{Value, json};

struct Fixture<W> {
    stdout: W,
    next_thread: u64,
    pending_tool: bool,
    parallel_agents: Option<ParallelAgents>,
    parallel_thread_id: Option<String>,
    team_guidance: bool,
    orchestrator_mode: bool,
    command_capable: bool,
    agent_schema_authoritative: bool,
    disconnected_tool_drained: bool,
    disconnect_marker: Option<PathBuf>,
}

#[derive(Default)]
struct ParallelAgents {
    agent_ids: Vec<String>,
    task_outputs: usize,
}

impl<W: Write> Fixture<W> {
    fn send(&mut self, message: Value) {
        writeln!(self.stdout, "{message}").expect("write mock app-server message");
        self.stdout.flush().expect("flush mock app-server message");
    }

    fn thread_id(&self) -> String {
        format!("thread-{}", self.next_thread)
    }

    fn handle(&mut self, message: &Value) -> bool {
        match message.get("method").and_then(Value::as_str) {
            Some("initialize") => self.send(json!({
                "id":message["id"], "result":{"userAgent":"codex-mock"}
            })),
            Some("initialized") => {}
            Some("force/error") => self.send(json!({
                "id":message["id"], "error":{"code":-32000,"message":"forced"}
            })),
            Some("force/exit") => return false,
            Some("thread/start") => {
                let instructions = message
                    .pointer("/params/developerInstructions")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.team_guidance = instructions.contains("named teammate's name");
                self.orchestrator_mode =
                    instructions.contains("main-session orchestration mode is active");
                self.command_capable = command_capability(message);
                self.agent_schema_authoritative = agent_schema_is_authoritative(message);
                self.next_thread += 1;
                self.send(json!({
                    "id":message["id"], "result":{"thread":{"id":self.thread_id()}}
                }));
            }
            Some("turn/start") => self.start_turn(message),
            None => self.handle_tool_result(message),
            _ => {}
        }
        true
    }

    fn start_turn(&mut self, message: &Value) {
        const MAX_INPUT_CHARS: usize = 1_048_576;
        let input = message
            .pointer("/params/input")
            .unwrap_or(&Value::Null)
            .to_string();
        if input.chars().count() > MAX_INPUT_CHARS {
            self.send(json!({
                "id":message["id"],
                "error":{
                    "code":-32602,
                    "data":{"input_error_code":"input_too_large","max_chars":MAX_INPUT_CHARS},
                    "message":"Input exceeds the maximum length"
                }
            }));
            return;
        }
        if input.contains("DETACHED_ERROR") {
            self.send(json!({
                "id":message["id"],
                "error":{"code":-32001,"message":"detached failure"}
            }));
        } else if !input.contains("STREAMING_DELAY") {
            self.send(json!({
                "id":message["id"], "result":{"turn":{"id":"turn-test"}}
            }));
        }
        self.send(json!({
            "method":"fixture/ignored", "params":{"threadId":"other-thread"}
        }));
        self.run_scenario(message, &input);
    }

    fn run_scenario(&mut self, message: &Value, input: &str) {
        if input.contains("DETACHED_ERROR") {
            return;
        }
        if self.run_turn_scenario(message, input)
            || self.run_control_scenario(message, input)
            || self.run_parallel_scenario(message, input)
            || self.run_disconnect_scenario(input)
        {
            return;
        }
        if let Some(tool) = requested_tool(input) {
            self.send_tool(tool, input);
        } else {
            self.send_plain_or_streamed(message, input);
        }
    }

    fn run_turn_scenario(&mut self, message: &Value, input: &str) -> bool {
        if input.contains("CONTEXT_WINDOW_ONCE") {
            self.context_window_once(message);
        } else if input.contains("CONTEXT_WINDOW_ALWAYS") {
            self.context_window_error(message);
        } else if input.contains("USAGE_LIMIT_ONCE") {
            self.usage_limit_once(message);
        } else if input.contains("USAGE_LIMIT_ALWAYS") {
            self.usage_limit_error(message);
        } else if input.contains("AUTH_FAIL_ALWAYS") {
            self.auth_failure_error(message);
        } else if input.contains("RETRY_THEN_OK") {
            self.retry_then_ok();
        } else if input.contains("TURN_FAILED") {
            self.complete_with_status("failed");
        } else if input.contains("TURN_ERROR") {
            self.send(json!({
                "method":"error",
                "params":{"threadId":self.thread_id(),"message":"forced turn error"}
            }));
        } else if input.contains("REPORT_EFFORT") {
            let effort = message
                .pointer("/params/effort")
                .and_then(Value::as_str)
                .unwrap_or("unset");
            self.send_text_and_complete(effort);
        } else if input.contains("VERIFY_AGENT_SCHEMA_AUTHORITY") {
            self.send_text_and_complete(agent_schema_response(self.agent_schema_authoritative));
        } else if input.contains("WEBSEARCH_EMPTY") {
            self.send(json!({
                "method":"turn/completed",
                "params":{"threadId":self.thread_id(), "turnId":"turn-test"}
            }));
        } else if input.contains("WEBSEARCH_QUERY") {
            self.send_web_search();
        } else if input.contains("USE_NAMED_TEAM_MAILBOX") {
            self.send_named_teammate();
        } else {
            return false;
        }
        true
    }

    fn run_control_scenario(&mut self, message: &Value, input: &str) -> bool {
        if input.contains("FOLLOW_UP_LAUNCH_AGENT") {
            self.send_control_tool(
                message,
                "call-follow-up-agent",
                "cc_Agent_0",
                json!({
                    "description":"follow-up implementation",
                    "prompt":"perform the newly requested independent follow-up",
                    "subagent_type":"general-purpose",
                    "run_in_background":true,
                    "claudex_model":"test-main-model"
                }),
            );
        } else if input.contains("FOLLOW_UP_REUSE_AGENT") {
            self.send_control_tool(
                message,
                "call-follow-up-reuse",
                "cc_TaskOutput_1",
                json!({
                    "task_id":"agent-profile-7",
                    "block":false,
                    "timeout":0
                }),
            );
        } else if input.contains("FOLLOW_UP_NO_AGENT") {
            self.send_text_and_complete("FOLLOW_UP_NO_AGENT_LAUNCHED");
        } else if input.contains("CONTROL_SUBAGENTS_TASK_OUTPUT") {
            self.send_control_tool(
                message,
                "call-control-output",
                "cc_TaskOutput_1",
                json!({"task_id":"agent-profile-7", "block":true, "timeout":120000}),
            );
        } else if input.contains("CONTROL_SUBAGENTS_SEND_MESSAGE") {
            self.send_control_tool(
                message,
                "call-control-message",
                "cc_TaskOutput_1",
                json!({
                    "task_id":"agent-profile-7",
                    "block":true,
                    "timeout":120000
                }),
            );
        } else if input.contains("CONTROL_SUBAGENTS_TASK_UPDATE") {
            self.send_control_tool(
                message,
                "call-control-update",
                "cc_TaskUpdate_4",
                json!({
                    "task_id":"agent-profile-7",
                    "status":"in_progress",
                    "description":"Revise the report with the latest findings."
                }),
            );
        } else if input.contains("CONTROL_SUBAGENTS_CONTINUE") {
            self.send_text_and_complete(orchestration_response(self.orchestrator_mode));
        } else if input.contains("CONTROL_SUBAGENTS_STOP") {
            self.send_control_tool(
                message,
                "call-control-stop",
                "cc_TaskStop_2",
                json!({
                    "task_id":"agent-profile-7",
                    "reason":"user requested that the main session stop this SubAgent"
                }),
            );
        } else {
            return false;
        }
        true
    }

    fn run_parallel_scenario(&mut self, message: &Value, input: &str) -> bool {
        if input.contains("USE_PARALLEL_AGENTS_TASK_OUTPUT") {
            self.send_parallel_agents(message);
        } else if input.contains("USE_PARALLEL_TOOLS") {
            self.send_delayed_parallel_tools();
        } else if input.contains("USE_INTERLEAVED_TOOLS") {
            self.send_interleaved_tools();
        } else if input.contains("TEXT_THEN_TOOL") {
            self.send_text_then_tool();
        } else if input.contains("PROVIDER_TOOL_PROGRESS") {
            self.send_provider_tool_progress();
        } else {
            return false;
        }
        true
    }

    fn run_disconnect_scenario(&mut self, input: &str) -> bool {
        if input.contains("DISCONNECT_WITH") {
            self.start_disconnected_tool_turn(input);
        } else if input.contains("REPORT_DISCONNECT_DRAIN") {
            self.send_text_and_complete(disconnect_drain_response(self.disconnected_tool_drained));
        } else if input.contains("RECOVER_ORPHAN_TOOL_RESULT")
            && input.contains(r#"\"type\":\"tool_result\""#)
        {
            self.send_text_and_complete("RECOVERED_ORPHAN_TOOL_RESULT");
        } else {
            return false;
        }
        true
    }

    fn send_web_search(&mut self) {
        let item = json!({
            "type":"webSearch", "query":"WEBSEARCH_QUERY",
            "results":[
                {"title":"Example result", "url":"https://example.com/source", "snippet":"fixture"},
                {"title":"Blocked result", "url":"https://blocked.example.com/source"}
            ]
        });
        self.send(json!({
            "method":"item/started",
            "params":{"threadId":self.thread_id(), "item":item}
        }));
        self.send(json!({
            "method":"item/completed",
            "params":{"threadId":self.thread_id(), "item":item}
        }));
        self.send(json!({
            "method":"turn/completed",
            "params":{"threadId":self.thread_id(), "turnId":"turn-test"}
        }));
    }

    fn start_disconnected_tool_turn(&mut self, input: &str) {
        // turn/start serializes `input` with Value::to_string(), so the prompt is
        // JSON-encoded and real newlines become the two characters '\' 'n'.
        self.disconnect_marker = disconnect_marker_from_input(input);
        self.pending_tool = true;
        self.send_text("DISCONNECT_READY");
        let delay = if input.contains("SLOW_TOOL") {
            500
        } else {
            100
        };
        thread::sleep(Duration::from_millis(delay));
        self.send_tool_event(900, "call-disconnected");
    }

    fn context_window_once(&mut self, message: &Value) {
        if self.next_thread == 1 {
            self.context_window_error(message);
        } else {
            self.send_text_and_complete("OK_AFTER_CONTEXT_RESTART");
        }
    }

    fn context_window_error(&mut self, message: &Value) {
        self.send(json!({
            "method":"error",
            "params":{
                "threadId":message.pointer("/params/threadId"),
                "turnId":"turn-test", "willRetry":false,
                "error":{
                    "codexErrorInfo":"contextWindowExceeded",
                    "message":"Codex ran out of room in the model's context window."
                }
            }
        }));
    }

    fn usage_limit_once(&mut self, message: &Value) {
        if self.next_thread == 1 {
            self.usage_limit_error(message);
        } else {
            self.send_text_and_complete("OK_AFTER_USAGE_LIMIT_FAILOVER");
        }
    }

    fn usage_limit_error(&mut self, message: &Value) {
        self.send(json!({
            "method":"error",
            "params":{
                "threadId":message.pointer("/params/threadId"),
                "turnId":"turn-test", "willRetry":false,
                "error":{
                    "codexErrorInfo":"usageLimitExceeded",
                    "message":"You've hit your usage limit. Try again at 3:20 AM."
                }
            }
        }));
    }

    fn auth_failure_error(&mut self, message: &Value) {
        self.send(json!({
            "method":"error",
            "params":{
                "threadId":message.pointer("/params/threadId"),
                "turnId":"turn-test", "willRetry":false,
                "error":{
                    "codexErrorInfo":"other",
                    "message":"unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
                }
            }
        }));
    }

    fn retry_then_ok(&mut self) {
        self.send(json!({
            "method":"error",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test", "willRetry":true,
                "error":{"message":"retry fixture"}
            }
        }));
        self.send_text_and_complete("OK_AFTER_RETRY");
    }

    fn send_delayed_parallel_tools(&mut self) {
        self.pending_tool = true;
        self.send_tool_event(900, "call-test-a");
        thread::sleep(Duration::from_millis(50));
        self.send_tool_event(901, "call-test-b");
    }

    fn send_parallel_agents(&mut self, message: &Value) {
        self.pending_tool = true;
        self.parallel_agents = Some(ParallelAgents::default());
        self.parallel_thread_id = message
            .pointer("/params/threadId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        for (id, name) in [(910, "profile"), (911, "business"), (912, "funding")] {
            self.send(json!({
                "id":id, "method":"item/tool/call",
                "params":{
                    "threadId":self.thread_id(), "turnId":"turn-test",
                    "callId":format!("call-agent-{name}"), "tool":"cc_Agent_0",
                    "arguments":{
                        "description":name, "prompt":format!("research {name}"),
                        "subagent_type":"general-purpose", "run_in_background":true,
                        "claudex_model":"test-main-model"
                    }
                }
            }));
        }
    }

    fn send_control_tool(&mut self, message: &Value, call_id: &str, tool: &str, arguments: Value) {
        let thread_id = message.pointer("/params/threadId").and_then(Value::as_str);
        if self.parallel_agents.is_some() && self.parallel_thread_id.as_deref() == thread_id {
            self.send(json!({
                "method":"error",
                "params":{
                    "threadId":message.pointer("/params/threadId"),
                    "turnId":"turn-test", "willRetry":false,
                    "error":{"message":"active SubAgent turn still owns this thread"}
                }
            }));
            return;
        }
        self.pending_tool = true;
        self.send(json!({
            "id":900, "method":"item/tool/call",
            "params":{
                "threadId":thread_id, "turnId":"turn-test",
                "callId":call_id, "tool":tool, "arguments":arguments
            }
        }));
        self.send_response_item_completed(call_id, tool);
    }

    fn send_named_teammate(&mut self) {
        self.pending_tool = true;
        self.send(json!({
            "id":930, "method":"item/tool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "callId":"call-named-agent", "tool":"cc_Agent_0",
                "arguments":{
                    "description":"company profile", "prompt":"research profile",
                    "subagent_type":"general-purpose", "run_in_background":true,
                    "name":"company-profile", "claudex_model":"test-main-model"
                }
            }
        }));
    }

    fn send_interleaved_tools(&mut self) {
        self.pending_tool = true;
        for (id, call_id) in [(900, "call-test-a"), (901, "call-test-b")] {
            self.send_tool_event(id, call_id);
            self.send_response_item_completed(call_id, "lookup");
        }
    }

    fn send_tool_event(&mut self, id: u64, call_id: &str) {
        self.send(json!({
            "id":id, "method":"item/tool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test", "callId":call_id,
                "tool":"lookup", "arguments":{"key":call_id}
            }
        }));
    }

    fn send_text_then_tool(&mut self) {
        self.pending_tool = true;
        self.send_text("BEFORE_TOOL");
        self.send(json!({
            "id":900, "method":"item/tool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "callId":"call-text-tool", "tool":"lookup", "arguments":{"key":"alpha"}
            }
        }));
        self.send_response_item_completed("call-text-tool", "lookup");
    }

    fn send_provider_tool_progress(&mut self) {
        const PROVIDER_CALL_ID: &str = "provider-read-config";
        self.send(json!({
            "method":"item/providerTool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "callId":PROVIDER_CALL_ID, "tool":"Read", "title":"Read config",
                "arguments":{"path":"CLAUDE.md"}
            }
        }));
        self.send(json!({
            "method":"item/providerTool/update",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "callId":PROVIDER_CALL_ID, "status":"completed", "title":"Read config",
                "output":"config loaded"
            }
        }));
        self.send_text_and_complete("CODEX_PROVIDER_PROGRESS_OK");
    }

    fn send_tool(&mut self, tool: &str, input: &str) {
        if tool.contains("Bash") {
            self.send_command_tool(tool);
            return;
        }
        self.pending_tool = true;
        let arguments = if tool.contains("Agent") {
            requested_agent_arguments(input)
        } else {
            json!({"key":"alpha","task":"small task"})
        };
        self.send(json!({
            "id":900, "method":"item/tool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test", "callId":"call-test",
                "tool":tool, "arguments":arguments
            }
        }));
        self.send_response_item_completed("call-test", tool);
    }

    fn send_command_tool(&mut self, tool: &str) {
        if !self.command_capable || !command_probe_succeeds() {
            self.send_text_and_complete("COMMAND_TOOL_UNAVAILABLE");
            return;
        }
        self.pending_tool = true;
        self.send(json!({
            "id":900, "method":"item/tool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test", "callId":"call-command",
                "tool":tool,
                "arguments":{"command":"command -v git >/dev/null && command -v gh >/dev/null && printf CLAUDEX_COMMAND_PROBE_OK"}
            }
        }));
        self.send_response_item_completed("call-command", tool);
    }

    fn send_plain_or_streamed(&mut self, message: &Value, input: &str) {
        if input.contains("OVERSIZED_IGNORED_EVENT") {
            self.send(json!({
                "method":"item/started",
                "params":{
                    "threadId":self.thread_id(),
                    "item":{"input":"x".repeat(2 * 1024 * 1024)}
                }
            }));
        }
        self.send(json!({
            "method":"fixture/ignored", "params":{"threadId":self.thread_id()}
        }));
        if input.contains("STREAMING_DELAY") {
            self.send_text("FIRST");
            thread::sleep(Duration::from_millis(200));
            self.send_text_and_complete("SECOND");
            self.send(json!({
                "id":message["id"], "result":{"turn":{"id":"turn-test"}}
            }));
        } else {
            self.send_text_and_complete("OK");
        }
    }

    fn handle_tool_result(&mut self, message: &Value) {
        if self.handle_named_teammate_result(message) {
            return;
        }
        if self.handle_parallel_agent_result(message) {
            return;
        }
        if !self.pending_tool || message.get("id") != Some(&json!(900)) {
            return;
        }
        self.pending_tool = false;
        let text = message
            .pointer("/result/contentItems/0/text")
            .and_then(Value::as_str)
            .unwrap_or("missing tool result")
            .to_owned();
        let disconnected_tool_drained = message.pointer("/result/success") == Some(&json!(false))
            && text.contains("disconnected");
        self.disconnected_tool_drained = disconnected_tool_drained;
        self.send_text_and_complete(&text);
        if disconnected_tool_drained {
            let marker = self
                .disconnect_marker
                .as_ref()
                .expect("disconnect marker path was not provided");
            fs::write(marker, "").expect("write disconnect drain marker");
        }
    }

    fn handle_named_teammate_result(&mut self, message: &Value) -> bool {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return false;
        };
        if id == 931 {
            self.pending_tool = false;
            self.send_text_and_complete("NAMED_TEAM_MAILBOX_COMPLETE");
            return true;
        }
        if id != 930 {
            return false;
        }
        let result = message
            .pointer("/result/contentItems/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if result.contains("DELAY_NAMED_RESULT") {
            thread::sleep(Duration::from_millis(250));
        }
        let protocol_ok = self.team_guidance
            && result.contains("not a TaskOutput")
            && result.contains("company-profile@session-fixture");
        let (tool, arguments) = if protocol_ok {
            (
                "cc_SendMessage_1",
                json!({
                    "to":"company-profile", "summary":"request final report",
                    "message":"Return the final report through the mailbox."
                }),
            )
        } else {
            ("cc_TaskOutput_2", json!({"task_id":"company-profile"}))
        };
        self.send(json!({
            "id":931, "method":"item/tool/call",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "callId":"call-team-follow-up", "tool":tool, "arguments":arguments
            }
        }));
        true
    }

    fn handle_parallel_agent_result(&mut self, message: &Value) -> bool {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return false;
        };
        if !(910..=912).contains(&id) && !(920..=922).contains(&id) {
            return false;
        }
        let text = message
            .pointer("/result/contentItems/0/text")
            .and_then(Value::as_str)
            .unwrap_or("missing parallel result")
            .to_owned();
        if (910..=912).contains(&id) {
            self.record_agent_id(text);
        } else {
            self.record_task_output();
        }
        true
    }

    fn record_agent_id(&mut self, agent_id: String) {
        let Some(workflow) = self.parallel_agents.as_mut() else {
            return;
        };
        workflow.agent_ids.push(agent_id);
        if workflow.agent_ids.len() != 3 {
            return;
        }
        let agent_ids = workflow.agent_ids.clone();
        for (offset, agent_id) in agent_ids.iter().enumerate() {
            self.send(json!({
                "id":920 + offset, "method":"item/tool/call",
                "params":{
                    "threadId":self.thread_id(), "turnId":"turn-test",
                    "callId":format!("call-output-{offset}"), "tool":"cc_TaskOutput_1",
                    "arguments":{"task_id":agent_id, "block":true, "timeout":120000}
                }
            }));
        }
    }

    fn record_task_output(&mut self) {
        let Some(workflow) = self.parallel_agents.as_mut() else {
            return;
        };
        workflow.task_outputs += 1;
        if workflow.task_outputs == 3 {
            self.pending_tool = false;
            self.parallel_agents = None;
            self.send_text_and_complete("PARALLEL_AGENT_RESULTS_COMPLETE");
        }
    }

    fn send_text_and_complete(&mut self, text: &str) {
        self.send_text(text);
        self.send_token_usage();
        self.complete_with_status("completed");
    }

    fn send_token_usage(&mut self) {
        self.send(json!({
            "method":"thread/tokenUsage/updated",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "tokenUsage":{
                    "last":{"inputTokens":17,"outputTokens":3,"cachedInputTokens":0,
                        "reasoningOutputTokens":0,"totalTokens":20},
                    "total":{"inputTokens":17,"outputTokens":3,"cachedInputTokens":0,
                        "reasoningOutputTokens":0,"totalTokens":20},
                    "modelContextWindow":200000
                }
            }
        }));
    }

    fn complete_with_status(&mut self, status: &str) {
        self.send(json!({
            "method":"turn/completed",
            "params":{
                "threadId":self.thread_id(),
                "turn":{"id":"turn-test","status":status}
            }
        }));
    }

    fn send_response_item_completed(&mut self, call_id: &str, tool: &str) {
        self.send(json!({
            "method":"rawResponseItem/completed",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "item":{
                    "type":"function_call", "name":tool,
                    "arguments":"{}", "call_id":call_id
                }
            }
        }));
    }

    fn send_text(&mut self, text: &str) {
        self.send(json!({
            "method":"item/agentMessage/delta",
            "params":{
                "threadId":self.thread_id(), "turnId":"turn-test",
                "itemId":"item-test", "delta":text
            }
        }));
    }
}

fn agent_schema_response(authoritative: bool) -> &'static str {
    if authoritative {
        "AGENT_SCHEMA_AUTHORITY_OK"
    } else {
        "AGENT_SCHEMA_AUTHORITY_MUTATED"
    }
}

fn orchestration_response(orchestrator_mode: bool) -> &'static str {
    if orchestrator_mode {
        "MAIN_RESPONSE_CONTINUED"
    } else {
        "WORKER_RESPONSE_MISROUTED"
    }
}

fn disconnect_drain_response(drained: bool) -> &'static str {
    if drained {
        "CODEX_DISCONNECT_DRAINED"
    } else {
        "CODEX_DISCONNECT_ABANDONED"
    }
}

fn requested_agent_arguments(input: &str) -> Value {
    if input.contains("USE_AGENT_DEFAULT") {
        return json!({
            "description":"default effort fixture",
            "prompt":"REPORT_EFFORT SUBSCRIPTION_ROUTE",
            "subagent_type":"claude", "model":"sonnet"
        });
    }
    let mut arguments = json!({
        "description":"effort fixture",
        "prompt":requested_agent_prompt(input),
        "subagent_type":"claude", "model":"sonnet",
        "claudex_effort":requested_agent_effort(input)
    });
    if let Some(model) = requested_agent_model(input) {
        arguments["claudex_model"] = json!(model);
    }
    arguments
}

fn requested_agent_prompt(input: &str) -> &'static str {
    if input.contains("USE_AGENT_MODEL_GPT_TOOL") {
        "USE_TOOL"
    } else {
        "REPORT_EFFORT SUBSCRIPTION_ROUTE"
    }
}

fn requested_tool(input: &str) -> Option<&'static str> {
    if input.contains("USE_ADVISOR_PUBLIC") {
        Some("cc_advisor_0")
    } else if input.contains("USE_COLLABORATOR_PUBLIC") {
        Some("cc_claude_collaborator_0")
    } else if input.contains("USE_ADVISOR") {
        Some("advisor")
    } else if input.contains("USE_COLLABORATOR") {
        Some("claude_collaborator")
    } else if input.contains("USE_AGENT") {
        Some("cc_Agent_0")
    } else if input.contains("USE_TOOL") {
        Some("lookup")
    } else if input.contains("USE_COMMAND_TOOL") {
        Some("cc_Bash_0")
    } else {
        None
    }
}

fn command_capability(message: &Value) -> bool {
    let features = message.pointer("/params/config/features");
    let shell_features_enabled = ["shell_tool", "unified_exec", "tool_search"]
        .into_iter()
        .all(|name| features.and_then(|features| features.get(name)) == Some(&json!(true)));
    let bash_schema_present = message
        .pointer("/params/dynamicTools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some("cc_Bash_0")
                    && tool
                        .get("description")
                        .and_then(Value::as_str)
                        .is_some_and(|description| description.contains("tool `Bash`"))
            })
        });
    shell_features_enabled && bash_schema_present
}

fn agent_schema_is_authoritative(message: &Value) -> bool {
    let expected = json!({
        "type":"object",
        "properties":{
            "prompt":{"type":"string","description":"schema-authority-sentinel"},
            "subagent_type":{"type":"string","enum":["general-purpose","Explore"]}
        },
        "required":["prompt"],
        "additionalProperties":false,
        "x-native-contract":{"version":220}
    });
    let Some(tools) = message
        .pointer("/params/dynamicTools")
        .and_then(Value::as_array)
    else {
        return false;
    };
    tools.len() == 1
        && tools[0].get("name").and_then(Value::as_str) == Some("cc_Agent_0")
        && tools[0].get("inputSchema") == Some(&expected)
        && !tools.iter().any(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains("Agent_batch"))
        })
}

fn command_probe_succeeds() -> bool {
    Command::new("sh")
        .args([
            "-c",
            "command -v git >/dev/null && command -v gh >/dev/null && printf CLAUDEX_COMMAND_PROBE_OK >/dev/null",
        ])
        .status()
        .is_ok_and(|status| status.success())
}

fn disconnect_marker_from_input(input: &str) -> Option<PathBuf> {
    let rest = input.split("DISCONNECT_MARKER=").nth(1)?;
    let path: String = rest
        .chars()
        .take_while(|c| !matches!(*c, '"' | '\\' | '\n' | '\r' | ']' | '}'))
        .collect();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn requested_agent_effort(input: &str) -> &'static str {
    ["low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .find(|effort| input.contains(&format!("EFFORT_{}", effort.to_uppercase())))
        .unwrap_or("mid")
}

fn requested_agent_model(input: &str) -> Option<&'static str> {
    if input.contains("USE_AGENT_MODEL_GPT") {
        Some("gpt-5.6-sol")
    } else if input.contains("USE_AGENT_MODEL_GROK") {
        Some("grok-4.5")
    } else if input.contains("USE_AGENT_MODEL") {
        Some("claude-opus-4-8")
    } else {
        None
    }
}

fn main() {
    let stdin = io::stdin();
    let mut fixture = Fixture {
        stdout: io::stdout().lock(),
        next_thread: 0,
        pending_tool: false,
        parallel_agents: None,
        parallel_thread_id: None,
        team_guidance: false,
        orchestrator_mode: false,
        command_capable: false,
        agent_schema_authoritative: false,
        disconnected_tool_drained: false,
        disconnect_marker: None,
    };
    writeln!(fixture.stdout, "not-json").expect("write malformed fixture line");
    fixture.send(json!({"id":99999,"result":{}}));
    for line in stdin.lock().lines() {
        let message = serde_json::from_str(&line.expect("read JSONL line"))
            .expect("parse adapter JSON-RPC request");
        if !fixture.handle(&message) {
            break;
        }
    }
}
