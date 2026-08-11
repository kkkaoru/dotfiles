use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use super::super::events::TurnResult;

pub(super) fn fallback_after_stdout(
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

pub(super) async fn read_stderr(stderr: impl AsyncRead + Unpin) -> String {
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
