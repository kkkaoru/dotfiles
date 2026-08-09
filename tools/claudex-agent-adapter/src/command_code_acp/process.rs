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
    let stderr_task = tokio::spawn(async move { read_stderr(stderr).await });
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
                let line = decoder.push_line(&bytes);
                if line.is_empty() && decoder.has_pending() {
                    continue;
                }
                match parse_stdout_line(&line) {
                    ParsedLine::Progress(event) => {
                        push_coalesced(&mut progress, sink.as_ref(), &mut coalescer, event)
                    }
                    ParsedLine::Result(parsed) => result = Some(parsed),
                    ParsedLine::Ignored => {}
                }
            }
            Err(error) => {
                stdout_error = Some(error);
                break;
            }
        }
    }
    if let Some(line) = decoder.flush() {
        match parse_stdout_line(&line) {
            ParsedLine::Progress(event) => {
                push_coalesced(&mut progress, sink.as_ref(), &mut coalescer, event)
            }
            ParsedLine::Result(parsed) => result = Some(parsed),
            ParsedLine::Ignored => {}
        }
    }
    for event in coalescer.finish() {
        push_progress(&mut progress, sink.as_ref(), event);
    }
    let status = child.wait().await.context("wait for cmd -p")?;
    let stderr_text = stderr_task.await.unwrap_or_else(|_| String::new());
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
            return if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(buf))
            };
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
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                out.push(String::from_utf8_lossy(&buf).into_owned());
            }
            Err(_) => break,
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::Utf8LineDecoder;

    #[test]
    fn decoder_replaces_invalid_bytes_and_keeps_following_json() {
        let mut decoder = Utf8LineDecoder::default();
        assert_eq!(decoder.push_line(b"\xff\n"), "\u{FFFD}");
        let result = decoder.push_line(
            br#"{"type":"result","subtype":"success","finalText":"AFTER_INVALID_UTF8"}"#,
        );
        assert!(result.contains("AFTER_INVALID_UTF8"));
        assert!(!decoder.has_pending());
    }

    #[test]
    fn decoder_carries_incomplete_utf8_across_reads() {
        let mut decoder = Utf8LineDecoder::default();
        assert!(decoder.push_line(&[0xe3]).is_empty());
        assert!(decoder.has_pending());
        assert_eq!(decoder.push_line(&[0x81, 0x82, b'\n']), "あ");
        assert!(!decoder.has_pending());
    }

    #[test]
    fn decoder_flush_lossy_decodes_trailing_incomplete_utf8() {
        let mut decoder = Utf8LineDecoder::default();
        assert!(decoder.push_line(&[0xe3]).is_empty());
        assert_eq!(decoder.flush().as_deref(), Some("\u{FFFD}"));
        assert!(!decoder.has_pending());
    }
}
