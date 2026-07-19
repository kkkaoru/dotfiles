use std::{
    io::{self, BufRead, Write},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

struct Fixture<W> {
    stdout: W,
    pending_tool: bool,
    parallel_agents: Option<ParallelAgents>,
    team_guidance: bool,
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
                self.team_guidance = message
                    .pointer("/params/developerInstructions")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("named teammate's name"));
                self.send(json!({
                    "id":message["id"], "result":{"thread":{"id":"thread-test"}}
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
        if input.contains("RETRY_THEN_OK") {
            self.retry_then_ok();
        } else if input.contains("TURN_FAILED") {
            self.complete_with_status("failed");
        } else if input.contains("TURN_ERROR") {
            self.send(json!({
                "method":"error",
                "params":{"threadId":"thread-test","message":"forced turn error"}
            }));
        } else if input.contains("REPORT_EFFORT") {
            let effort = message
                .pointer("/params/effort")
                .and_then(Value::as_str)
                .unwrap_or("unset");
            self.send_text_and_complete(effort);
        } else if input.contains("USE_NAMED_TEAM_MAILBOX") {
            self.send_named_teammate();
        } else if input.contains("USE_PARALLEL_AGENTS_TASK_OUTPUT") {
            self.send_parallel_agents();
        } else if input.contains("USE_PARALLEL_TOOLS") {
            self.send_delayed_parallel_tools();
        } else if input.contains("USE_INTERLEAVED_TOOLS") {
            self.send_interleaved_tools();
        } else if input.contains("TEXT_THEN_TOOL") {
            self.send_text_then_tool();
        } else if input.contains("RECOVER_ORPHAN_TOOL_RESULT")
            && input.contains(r#"\"type\":\"tool_result\""#)
        {
            self.send_text_and_complete("RECOVERED_ORPHAN_TOOL_RESULT");
        } else if let Some(tool) = requested_tool(input) {
            self.send_tool(tool, input);
        } else {
            self.send_plain_or_streamed(message, input);
        }
    }

    fn retry_then_ok(&mut self) {
        self.send(json!({
            "method":"error",
            "params":{
                "threadId":"thread-test", "turnId":"turn-test", "willRetry":true,
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

    fn send_parallel_agents(&mut self) {
        self.pending_tool = true;
        self.parallel_agents = Some(ParallelAgents::default());
        for (id, name) in [(910, "profile"), (911, "business"), (912, "funding")] {
            self.send(json!({
                "id":id, "method":"item/tool/call",
                "params":{
                    "threadId":"thread-test", "turnId":"turn-test",
                    "callId":format!("call-agent-{name}"), "tool":"cc_Agent_0",
                    "arguments":{
                        "description":name, "prompt":format!("research {name}"),
                        "subagent_type":"general-purpose", "run_in_background":true
                    }
                }
            }));
        }
    }

    fn send_named_teammate(&mut self) {
        self.pending_tool = true;
        self.send(json!({
            "id":930, "method":"item/tool/call",
            "params":{
                "threadId":"thread-test", "turnId":"turn-test",
                "callId":"call-named-agent", "tool":"cc_Agent_0",
                "arguments":{
                    "description":"company profile", "prompt":"research profile",
                    "subagent_type":"general-purpose", "run_in_background":true,
                    "name":"company-profile"
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
                "threadId":"thread-test", "turnId":"turn-test", "callId":call_id,
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
                "threadId":"thread-test", "turnId":"turn-test",
                "callId":"call-text-tool", "tool":"lookup", "arguments":{"key":"alpha"}
            }
        }));
        self.send_response_item_completed("call-text-tool", "lookup");
    }

    fn send_tool(&mut self, tool: &str, input: &str) {
        self.pending_tool = true;
        let arguments = if tool.contains("Agent") && input.contains("USE_AGENT_DEFAULT") {
            json!({
                "description":"default effort fixture",
                "prompt":"REPORT_EFFORT SUBSCRIPTION_ROUTE",
                "subagent_type":"claude", "model":"sonnet"
            })
        } else if tool.contains("Agent") {
            let prompt = if input.contains("USE_AGENT_MODEL_GPT_TOOL") {
                "USE_TOOL"
            } else {
                "REPORT_EFFORT SUBSCRIPTION_ROUTE"
            };
            let mut arguments = json!({
                "description":"effort fixture",
                "prompt":prompt,
                "subagent_type":"claude", "model":"sonnet",
                "claudex_effort":requested_agent_effort(input)
            });
            if let Some(model) = requested_agent_model(input) {
                arguments["claudex_model"] = json!(model);
            }
            arguments
        } else {
            json!({"key":"alpha","task":"small task"})
        };
        self.send(json!({
            "id":900, "method":"item/tool/call",
            "params":{
                "threadId":"thread-test", "turnId":"turn-test", "callId":"call-test",
                "tool":tool, "arguments":arguments
            }
        }));
        self.send_response_item_completed("call-test", tool);
    }

    fn send_plain_or_streamed(&mut self, message: &Value, input: &str) {
        if input.contains("OVERSIZED_IGNORED_EVENT") {
            self.send(json!({
                "method":"item/started",
                "params":{
                    "threadId":"thread-test",
                    "item":{"input":"x".repeat(2 * 1024 * 1024)}
                }
            }));
        }
        self.send(json!({
            "method":"fixture/ignored", "params":{"threadId":"thread-test"}
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
        self.send_text_and_complete(&text);
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
                "threadId":"thread-test", "turnId":"turn-test",
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
                    "threadId":"thread-test", "turnId":"turn-test",
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
                "threadId":"thread-test", "turnId":"turn-test",
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
                "threadId":"thread-test",
                "turn":{"id":"turn-test","status":status}
            }
        }));
    }

    fn send_response_item_completed(&mut self, call_id: &str, tool: &str) {
        self.send(json!({
            "method":"rawResponseItem/completed",
            "params":{
                "threadId":"thread-test", "turnId":"turn-test",
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
                "threadId":"thread-test", "turnId":"turn-test",
                "itemId":"item-test", "delta":text
            }
        }));
    }
}

fn requested_tool(input: &str) -> Option<&'static str> {
    if input.contains("USE_ADVISOR") {
        Some("advisor")
    } else if input.contains("USE_COLLABORATOR") {
        Some("claude_collaborator")
    } else if input.contains("USE_AGENT") {
        Some("cc_Agent_0")
    } else if input.contains("USE_TOOL") {
        Some("lookup")
    } else {
        None
    }
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
        pending_tool: false,
        parallel_agents: None,
        team_guidance: false,
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
