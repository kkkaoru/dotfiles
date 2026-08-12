use std::path::Path;

use anyhow::{Context, Result};
use tokio::{process::Child, sync::mpsc};

use super::events::{ProgressEvent, TurnResult};
use super::launch::LaunchSpec;

mod decode;
mod fallback;
#[cfg(test)]
use decode::Utf8LineDecoder;
use fallback::{fallback_after_stdout, read_stderr};
use helpers::{drain_stdout, spawn_cmd, terminate_process_group, trace_spawn};

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
    let child = spawn_cmd(spec, &argv, cwd)?;
    let process_group = child.id();
    let mut child = ProcessChild::new(child, process_group);
    let stdout = child
        .child
        .stdout
        .take()
        .context("command-code headless stdout is unavailable")?;
    let stderr = child
        .child
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
    let status = child.child.wait().await.context("wait for cmd -p")?;
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

struct ProcessChild {
    child: Child,
    process_group: Option<u32>,
}

impl ProcessChild {
    fn new(child: Child, process_group: Option<u32>) -> Self {
        Self {
            child,
            process_group,
        }
    }
}

impl Drop for ProcessChild {
    fn drop(&mut self) {
        if let Some(process_group) = self.process_group {
            terminate_process_group(process_group);
        }
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
