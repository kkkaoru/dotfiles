mod app;
mod collect;
mod config;
mod exhaustion;
mod hook;
mod lifecycle;
mod opencode_go_budget;
mod process;
mod refresh;
mod routing;
mod snapshot;
mod trusted;
mod util;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use serde_json::Value;
use std::io::{self, IsTerminal, Read as _};
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "PascalCase")]
enum HookEvent {
    UserPromptSubmit,
    SubagentStart,
}

impl HookEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SubagentStart => "SubagentStart",
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Emit sanitized routing context from Codexbar usage")]
struct Arguments {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    disabled_models_config: Option<PathBuf>,
    #[arg(long)]
    input: Option<PathBuf>,
    #[arg(long)]
    no_cache: bool,
    #[arg(long, default_value = "codexbar")]
    codexbar_program: String,
    #[arg(long, default_value = "curl")]
    curl_program: String,
    #[arg(long, value_enum, default_value = "UserPromptSubmit")]
    event: HookEvent,
    #[arg(long, hide = true)]
    refresh_cache_worker: bool,
    #[arg(long, hide = true)]
    refresh_lock_fd: Option<RawFd>,
    #[arg(long, hide = true)]
    refresh_ticket_fd: Option<RawFd>,
}

impl Arguments {
    fn explicit_diagnostic(&self) -> bool {
        self.no_cache || self.input.is_some()
    }
}

#[derive(Debug, PartialEq)]
enum Invocation {
    Manual,
    Hook(Value),
    InvalidHook,
}

fn parse_hook_stdin(raw: &str) -> Invocation {
    if raw.trim().is_empty() {
        return Invocation::InvalidHook;
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value) if value.is_object() => Invocation::Hook(value),
        _ => Invocation::InvalidHook,
    }
}

fn read_invocation() -> Result<Invocation> {
    if io::stdin().is_terminal() {
        return Ok(Invocation::Manual);
    }
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    Ok(parse_hook_stdin(&raw))
}

trait HookRuntime {
    fn normal(&self, arguments: &Arguments, payload: Option<&Value>) -> Result<Value>;
}

struct ProductionRuntime;

impl HookRuntime for ProductionRuntime {
    fn normal(&self, arguments: &Arguments, payload: Option<&Value>) -> Result<Value> {
        app::normal_hook_output(arguments, payload)
    }
}

fn execute<R: HookRuntime>(
    arguments: &Arguments,
    invocation: &Invocation,
    runtime: &R,
) -> Result<Value> {
    if arguments.explicit_diagnostic() {
        let payload = match invocation {
            Invocation::Hook(value) => Some(value),
            Invocation::Manual | Invocation::InvalidHook => None,
        };
        return runtime.normal(arguments, payload);
    }
    match invocation {
        Invocation::Manual => runtime.normal(arguments, None),
        Invocation::InvalidHook => Ok(serde_json::json!({})),
        Invocation::Hook(payload)
            if arguments.event == HookEvent::UserPromptSubmit
                && lifecycle::is_lifecycle_notification(payload) =>
        {
            Ok(serde_json::json!({}))
        }
        Invocation::Hook(payload)
            if arguments.event == HookEvent::UserPromptSubmit
                && lifecycle::prompt(payload).is_none_or(|prompt| prompt.trim().is_empty()) =>
        {
            Ok(serde_json::json!({}))
        }
        Invocation::Hook(payload) => runtime.normal(arguments, Some(payload)),
    }
}

fn run() -> Result<()> {
    let arguments = Arguments::parse();
    if arguments.refresh_cache_worker {
        return app::refresh_cache_worker(&arguments);
    }
    if arguments.refresh_lock_fd.is_some() || arguments.refresh_ticket_fd.is_some() {
        return Ok(());
    }
    let invocation = read_invocation()?;
    let output = execute(&arguments, &invocation, &ProductionRuntime)?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct CountingRuntime {
        config_reads: Cell<usize>,
        cache_reads: Cell<usize>,
        process_starts: Cell<usize>,
    }

    impl CountingRuntime {
        fn new() -> Self {
            Self {
                config_reads: Cell::new(0),
                cache_reads: Cell::new(0),
                process_starts: Cell::new(0),
            }
        }
    }

    impl HookRuntime for CountingRuntime {
        fn normal(&self, _arguments: &Arguments, _payload: Option<&Value>) -> Result<Value> {
            self.config_reads.set(self.config_reads.get() + 1);
            self.cache_reads.set(self.cache_reads.get() + 1);
            self.process_starts.set(self.process_starts.get() + 1);
            Ok(serde_json::json!({"routed": true}))
        }
    }

    fn arguments() -> Arguments {
        Arguments::try_parse_from(["route-usage"]).unwrap()
    }

    #[test]
    fn production_entrypoint_skips_all_runtime_io_for_prompt_only_incident() {
        let runtime = CountingRuntime::new();
        let payload = serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": concat!(
                "<task-notification>\n",
                "<task-id>a760b564e16f0c75b</task-id>\n",
                "<status>completed</status>\n",
                "<summary>Agent \"worker\" finished</summary>\n",
                "</task-notification>"
            )
        });
        let output = execute(&arguments(), &Invocation::Hook(payload), &runtime).unwrap();
        assert_eq!(output, serde_json::json!({}));
        assert_eq!(runtime.config_reads.get(), 0);
        assert_eq!(runtime.cache_reads.get(), 0);
        assert_eq!(runtime.process_starts.get(), 0);
    }

    #[test]
    fn invalid_empty_nonobject_and_prompt_type_inputs_are_immediate_success() {
        let runtime = CountingRuntime::new();
        for invocation in [
            parse_hook_stdin(""),
            parse_hook_stdin("{broken"),
            parse_hook_stdin("[]"),
            parse_hook_stdin("null"),
            parse_hook_stdin("\"text\""),
            parse_hook_stdin("{}"),
            parse_hook_stdin(r#"{"prompt":42}"#),
            parse_hook_stdin(r#"{"prompt":""}"#),
            parse_hook_stdin(r#"{"prompt":"　\t"}"#),
            parse_hook_stdin(
                r#"{"prompt":null,"user_prompt":"<task-notification>done</task-notification>"}"#,
            ),
        ] {
            assert_eq!(
                execute(&arguments(), &invocation, &runtime).unwrap(),
                serde_json::json!({})
            );
        }
        assert_eq!(runtime.config_reads.get(), 0);
        assert_eq!(runtime.cache_reads.get(), 0);
        assert_eq!(runtime.process_starts.get(), 0);
    }

    #[test]
    fn normal_prompt_uses_the_routing_runtime() {
        let runtime = CountingRuntime::new();
        let invocation = parse_hook_stdin(r#"{"prompt":"Please continue"}"#);
        assert_eq!(
            execute(&arguments(), &invocation, &runtime).unwrap(),
            serde_json::json!({"routed": true})
        );
        assert_eq!(runtime.config_reads.get(), 1);
    }
}
