use std::io;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::coalesce::ProgressCoalescer;
use super::events::{ParsedLine, ProgressEvent, TurnResult, parse_stdout_line};
use super::launch::LaunchSpec;

const MAX_STDOUT_LINE_BYTES: usize = 2 * 1024 * 1024;

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

async fn read_stdout_line(
    reader: &mut BufReader<impl AsyncRead + Unpin>,
) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(eof_stdout_buf(buf));
        }
        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            buf.extend_from_slice(&available[..=pos]);
            reader.consume(pos + 1);
            return Ok(Some(buf));
        }
        let take = available
            .len()
            .min(MAX_STDOUT_LINE_BYTES.saturating_sub(buf.len()));
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if buf.len() >= MAX_STDOUT_LINE_BYTES {
            return Ok(Some(buf));
        }
    }
}

fn eof_stdout_buf(buf: Vec<u8>) -> Option<Vec<u8>> {
    if buf.is_empty() { None } else { Some(buf) }
}

#[derive(Default)]
struct Utf8LineDecoder {
    pending: Vec<u8>,
}

impl Utf8LineDecoder {
    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn push_line(&mut self, bytes: &[u8]) -> String {
        let mut line = bytes;
        if line.last() == Some(&b'\n') {
            line = &line[..line.len() - 1];
        }
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(line);
        decode_utf8_with_pending(&mut self.pending, data)
    }

    fn flush(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.pending);
        let text = String::from_utf8_lossy(&data).into_owned();
        if text.is_empty() { None } else { Some(text) }
    }
}

fn decode_utf8_with_pending(pending: &mut Vec<u8>, data: Vec<u8>) -> String {
    match std::str::from_utf8(&data) {
        Ok(text) => text.to_owned(),
        Err(err) if err.error_len().is_none() => {
            let valid = err.valid_up_to();
            pending.extend_from_slice(&data[valid..]);
            String::from_utf8_lossy(&data[..valid]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(&data).into_owned(),
    }
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

fn fallback_after_stdout(
    exit_code: Option<i32>,
    success: bool,
    stderr: &str,
    stdout_error: Option<&io::Error>,
) -> TurnResult {
    if let Some(error) = stdout_error {
        let code = exit_code
            .map(|code| format!("exit {code}"))
            .unwrap_or_else(|| "terminated".to_owned());
        let mut message = format!("Command Code stdout closed: {error}");
        if !stderr.is_empty() {
            message.push('\n');
            message.push_str(stderr);
        }
        return TurnResult {
            subtype: "error".to_owned(),
            session_id: None,
            stop_reason: Some("error".to_owned()),
            final_text: String::new(),
            error: Some(format!("{message} ({code})")),
        };
    }
    fallback_result(exit_code, success, stderr)
}

fn fallback_result(exit_code: Option<i32>, success: bool, stderr: &str) -> TurnResult {
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
        _ if !stderr.is_empty() => stderr,
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

async fn read_stderr(stderr: impl AsyncRead + Unpin) -> String {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => out.push(trim_stderr_line(&buf)),
            Err(_) => break,
        }
    }
    out.join("\n")
}

fn trim_stderr_line(buf: &[u8]) -> String {
    let mut line = buf;
    if line.last() == Some(&b'\n') {
        line = &line[..line.len() - 1];
    }
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    String::from_utf8_lossy(line).into_owned()
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
