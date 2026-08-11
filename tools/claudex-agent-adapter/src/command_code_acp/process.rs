use std::io;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::coalesce::ProgressCoalescer;
use super::events::{ParsedLine, ProgressEvent, TurnResult, parse_stdout_line};
use super::launch::LaunchSpec;

mod decode;
mod fallback;

use decode::{Utf8LineDecoder, read_stdout_line};
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

async fn drain_stdout(
    spec: &LaunchSpec,
    sink: Option<mpsc::UnboundedSender<ProgressEvent>>,
    stdout: impl AsyncRead + Unpin,
) -> (Vec<ProgressEvent>, Option<TurnResult>, Option<io::Error>) {
    let mut reader = BufReader::new(stdout);
    let mut progress = Vec::new();
    let mut coalescer = ProgressCoalescer::default();
    push_coalesced(
        &mut progress,
        sink.as_ref(),
        &mut coalescer,
        ProgressEvent::Started {
            model: spec.model.clone(),
            effort: spec.effort.clone(),
        },
    );
    let mut result = None;
    let mut stdout_error = None;
    let mut decoder = Utf8LineDecoder::default();
    loop {
        match read_stdout_line(&mut reader).await {
            Ok(None) => break,
            Ok(Some(bytes)) => {
                apply_stdout_bytes(
                    &mut progress,
                    sink.as_ref(),
                    &mut coalescer,
                    &mut decoder,
                    &mut result,
                    &bytes,
                );
            }
            Err(error) => {
                stdout_error = Some(error);
                break;
            }
        }
    }
    if let Some(line) = decoder.flush() {
        apply_parsed_line(
            &mut progress,
            sink.as_ref(),
            &mut coalescer,
            &mut result,
            parse_stdout_line(&line),
        );
    }
    for event in coalescer.finish() {
        push_progress(&mut progress, sink.as_ref(), event);
    }
    (progress, result, stdout_error)
}

fn apply_stdout_bytes(
    progress: &mut Vec<ProgressEvent>,
    sink: Option<&mpsc::UnboundedSender<ProgressEvent>>,
    coalescer: &mut ProgressCoalescer,
    decoder: &mut Utf8LineDecoder,
    result: &mut Option<TurnResult>,
    bytes: &[u8],
) {
    let line = decoder.push_line(bytes);
    if line.is_empty() && decoder.has_pending() {
        return;
    }
    apply_parsed_line(
        progress,
        sink,
        coalescer,
        result,
        parse_stdout_line(&line),
    );
}

fn apply_parsed_line(
    progress: &mut Vec<ProgressEvent>,
    sink: Option<&mpsc::UnboundedSender<ProgressEvent>>,
    coalescer: &mut ProgressCoalescer,
    result: &mut Option<TurnResult>,
    parsed: ParsedLine,
) {
    match parsed {
        ParsedLine::Progress(event) => push_coalesced(progress, sink, coalescer, event),
        ParsedLine::Result(parsed) => *result = Some(parsed),
        ParsedLine::Ignored => {}
    }
}

fn push_coalesced(
    progress: &mut Vec<ProgressEvent>,
    sink: Option<&mpsc::UnboundedSender<ProgressEvent>>,
    coalescer: &mut ProgressCoalescer,
    event: ProgressEvent,
) {
    for event in coalescer.push(event) {
        push_progress(progress, sink, event);
    }
}

fn push_progress(
    progress: &mut Vec<ProgressEvent>,
    sink: Option<&mpsc::UnboundedSender<ProgressEvent>>,
    event: ProgressEvent,
) {
    if let Some(sink) = sink {
        let _ = sink.send(event.clone());
    }
    progress.push(event);
}

fn trace_spawn(argv: &[String], cwd: Option<&Path>) {
    let Ok(path) = std::env::var("CLAUDEX_COMMAND_CODE_TRACE") else {
        return;
    };
    if path.trim().is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "cwd": cwd.map(|path| path.display().to_string()),
        "argv": argv,
    });
    let _ = std::fs::write(path, format!("{payload}\n"));
}

fn spawn_cmd(spec: &LaunchSpec, argv: &[String], cwd: Option<&Path>) -> Result<Child> {
    let mut command = Command::new(spec.program());
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.spawn().with_context(|| {
        format!(
            "failed to spawn Command Code headless: {} {}",
            spec.program().display(),
            argv.join(" ")
        )
    })
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
