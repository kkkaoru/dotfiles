use std::path::Path;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use super::events::{ProgressEvent, TurnResult};
use super::launch::LaunchSpec;

mod decode;
mod fallback;
#[cfg(test)]
use decode::Utf8LineDecoder;
use fallback::{fallback_after_stdout, read_stderr};

pub(super) const MAX_STDOUT_LINE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
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
    run_turn_emitting(spec, prompt, resume, None, None).await
}

pub async fn run_turn_emitting(
    spec: &LaunchSpec,
    prompt: &str,
    resume: Option<&str>,
    sink: Option<mpsc::UnboundedSender<ProgressEvent>>,
    cwd: Option<&Path>,
) -> Result<TurnOutcome> {
    let argv = spec.argv(prompt, resume);
    trace_spawn(&argv, cwd);
    let mut child = spawn_cmd(spec, &argv, cwd)?;
    let stdout = child
        .stdout
        .take()
        .context("command-code headless stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("command-code headless stderr is unavailable")?;
    // current_thread + LocalSet (command-code-acp) does not poll `tokio::spawn`
    // while a local prompt is running. Drain stdout/stderr on this task instead
    // so a full stderr pipe cannot deadlock cmd, and llvm-cov/LocalSet cannot
    // strand the reader.
    let stdout_drain = drain_stdout(spec, sink, stdout);
    let ((progress, result, stdout_error), stderr_text) =
        tokio::join!(stdout_drain, read_stderr(stderr));
    let status = child.wait().await.context("wait for cmd -p")?;
    let exit_code = status.code();
    let result = result.unwrap_or_else(|| {
        fallback_after_stdout(
            exit_code,
            status.success(),
            stderr_text.trim(),
            stdout_error.as_ref(),
        )
    });
    Ok(TurnOutcome {
        progress,
        result,
        exit_code,
    })
}

mod helpers;
use helpers::{drain_stdout, spawn_cmd, trace_spawn};

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
