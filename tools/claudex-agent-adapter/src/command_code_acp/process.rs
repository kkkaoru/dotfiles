use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::events::{ParsedLine, ProgressEvent, TurnResult, parse_stdout_line};
use super::launch::LaunchSpec;

pub struct TurnOutcome {
    pub progress: Vec<ProgressEvent>,
    pub result: TurnResult,
    pub exit_code: Option<i32>,
}

pub async fn run_turn(
    spec: &LaunchSpec,
    prompt: &str,
    resume: Option<&str>,
) -> Result<TurnOutcome> {
    let argv = spec.argv(prompt, resume);
    let mut child = spawn_cmd(spec, &argv)?;
    let stdout = child
        .stdout
        .take()
        .context("command-code headless stdout is unavailable")?;
    let mut lines = BufReader::new(stdout).lines();
    let mut progress = vec![ProgressEvent::Started {
        model: spec.model.clone(),
        effort: spec.effort.clone(),
    }];
    let mut result = None;
    while let Some(line) = lines.next_line().await.context("read cmd -p stdout")? {
        match parse_stdout_line(&line) {
            ParsedLine::Progress(event) => progress.push(event),
            ParsedLine::Result(parsed) => result = Some(parsed),
            ParsedLine::Ignored => {}
        }
    }
    let status = child.wait().await.context("wait for cmd -p")?;
    let exit_code = status.code();
    let result = result.unwrap_or_else(|| fallback_result(exit_code, status.success()));
    Ok(TurnOutcome {
        progress,
        result,
        exit_code,
    })
}

fn spawn_cmd(spec: &LaunchSpec, argv: &[String]) -> Result<Child> {
    let mut command = Command::new(spec.program());
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command.spawn().with_context(|| {
        format!(
            "failed to spawn Command Code headless: {} {}",
            spec.program().display(),
            argv.join(" ")
        )
    })
}

fn fallback_result(exit_code: Option<i32>, success: bool) -> TurnResult {
    if success {
        return TurnResult {
            subtype: "success".to_owned(),
            session_id: None,
            stop_reason: Some("end_turn".to_owned()),
            final_text: String::new(),
            error: None,
        };
    }
    let code = exit_code
        .map(|code| format!("exit {code}"))
        .unwrap_or_else(|| "terminated".to_owned());
    let message = match exit_code {
        Some(3) => {
            "Command Code is not authenticated. Run `cmd login` or set COMMAND_CODE_API_KEY."
        }
        Some(8) => "Command Code headless hit --max-turns before a final answer.",
        Some(10) => "Command Code headless has insufficient credits.",
        _ => "Command Code headless failed without a JSON result.",
    };
    TurnResult {
        subtype: "error".to_owned(),
        session_id: None,
        stop_reason: Some("error".to_owned()),
        final_text: String::new(),
        error: Some(format!("{message} ({code})")),
    }
}
