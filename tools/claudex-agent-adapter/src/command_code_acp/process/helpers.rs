use std::io;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use super::super::coalesce::ProgressCoalescer;
use super::super::events::{ParsedLine, ProgressEvent, TurnResult, parse_stdout_line};
use super::super::launch::LaunchSpec;
use super::decode::{Utf8LineDecoder, read_stdout_line};

pub(super) async fn drain_stdout(
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

pub(super) fn apply_stdout_bytes(
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
    apply_parsed_line(progress, sink, coalescer, result, parse_stdout_line(&line));
}

pub(super) fn apply_parsed_line(
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

pub(super) fn push_coalesced(
    progress: &mut Vec<ProgressEvent>,
    sink: Option<&mpsc::UnboundedSender<ProgressEvent>>,
    coalescer: &mut ProgressCoalescer,
    event: ProgressEvent,
) {
    for event in coalescer.push(event) {
        push_progress(progress, sink, event);
    }
}

pub(super) fn push_progress(
    progress: &mut Vec<ProgressEvent>,
    sink: Option<&mpsc::UnboundedSender<ProgressEvent>>,
    event: ProgressEvent,
) {
    if let Some(sink) = sink {
        let _ = sink.send(event.clone());
    }
    progress.push(event);
}

pub(super) fn trace_spawn(argv: &[String], cwd: Option<&Path>) {
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

pub(super) fn spawn_cmd(spec: &LaunchSpec, argv: &[String], cwd: Option<&Path>) -> Result<Child> {
    let mut command = Command::new(spec.program());
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
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

pub(super) fn terminate_process_group(process_group: u32) {
    #[cfg(unix)]
    {
        let Ok(process_group) = i32::try_from(process_group) else {
            return;
        };
        let _ = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    let _ = process_group;
}
