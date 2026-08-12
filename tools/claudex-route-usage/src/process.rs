//! Bounded subprocess execution with complete Unix-session cleanup.

mod capture;
mod deadline;
mod session;

pub(crate) use deadline::Deadline;

use anyhow::{Context, Result, bail};
use capture::{PipeCapture, set_nonblocking};
use std::io::{self, Write as _};
use std::process::{ChildStdin, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_POLL: Duration = Duration::from_millis(10);
const CLEANUP_RESERVE: Duration = Duration::from_secs(2);
const FINAL_DRAIN_GRACE: Duration = Duration::from_millis(100);

enum WaitOutcome {
    Completed,
    TimedOut,
}

fn leader_exited(child: &std::process::Child) -> io::Result<bool> {
    let mut information = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            information.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    Ok(unsafe { information.si_pid() } != 0)
}

fn wait_for_leader(
    child: &std::process::Child,
    capture: &mut PipeCapture,
    input: &mut InputPipe<'_>,
    cutoff: Instant,
) -> io::Result<WaitOutcome> {
    loop {
        input.write_available()?;
        capture.drain_available()?;
        if leader_exited(child)? {
            return Ok(WaitOutcome::Completed);
        }
        let now = Instant::now();
        if now >= cutoff {
            return Ok(WaitOutcome::TimedOut);
        }
        capture.wait(cutoff.saturating_duration_since(now).min(CHILD_POLL))?;
    }
}

struct InputPipe<'a> {
    pipe: Option<ChildStdin>,
    bytes: &'a [u8],
    offset: usize,
}

impl InputPipe<'_> {
    fn write_available(&mut self) -> io::Result<()> {
        if self.offset == self.bytes.len() {
            self.pipe = None;
            return Ok(());
        }
        let Some(pipe) = self.pipe.as_mut() else {
            return Ok(());
        };
        let count = match pipe.write(&self.bytes[self.offset..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "subprocess stdin")),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        self.offset += count;
        if self.offset == self.bytes.len() {
            self.pipe = None;
        }
        Ok(())
    }
}

fn wait_for_exit(child: &std::process::Child, deadline: Deadline) -> io::Result<bool> {
    loop {
        if leader_exited(child)? {
            return Ok(true);
        }
        let Some(remaining) = deadline.remaining() else {
            return Ok(false);
        };
        thread::sleep(remaining.min(CHILD_POLL));
    }
}

fn reap_leader(child: &mut std::process::Child, deadline: Deadline) -> Result<ExitStatus> {
    if !wait_for_exit(child, deadline).context("observe subprocess leader exit")? {
        let _ = child.kill();
        if !wait_for_exit(child, deadline).context("observe killed subprocess leader exit")? {
            bail!("subprocess leader could not be killed and reaped before its deadline");
        }
    }
    child.wait().context("reap subprocess leader")
}

fn result_note<T, E: std::fmt::Display>(result: &std::result::Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => error.to_string(),
    }
}

fn cleanup_child(
    child: &mut std::process::Child,
    session_id: u32,
    deadline: Deadline,
) -> (Result<()>, Result<ExitStatus>) {
    let cleanup = session::terminate_session(session_id, deadline);
    let status = reap_leader(child, deadline);
    (cleanup, status)
}

fn run_before_input(
    mut command: Command,
    deadline: Deadline,
    runtime_cap: Duration,
    input: Option<&[u8]>,
) -> Result<(i32, String, String)> {
    let execution_cutoff = deadline
        .cutoff(runtime_cap, CLEANUP_RESERVE)
        .context("subprocess deadline has no cleanup reserve")?;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    session::configure_new_session(&mut command);
    let mut child = command.spawn().context("failed to spawn subprocess")?;
    let session_id = child.id();
    let mut input = InputPipe {
        pipe: child.stdin.take(),
        bytes: input.unwrap_or_default(),
        offset: 0,
    };
    if let Some(pipe) = input.pipe.as_ref()
        && let Err(error) = set_nonblocking(pipe)
    {
        let (cleanup, reap) = cleanup_child(&mut child, session_id, deadline);
        return Err(error).context(format!(
            "configure subprocess input; session_cleanup={}; reap={}",
            result_note(&cleanup),
            result_note(&reap)
        ));
    }
    let mut capture = match PipeCapture::from_child(&mut child) {
        Ok(capture) => capture,
        Err(error) => {
            let (cleanup, reap) = cleanup_child(&mut child, session_id, deadline);
            return Err(error).context(format!(
                "configure subprocess output capture; session_cleanup={}; reap={}",
                result_note(&cleanup),
                result_note(&reap)
            ));
        }
    };

    let wait = wait_for_leader(&child, &mut capture, &mut input, execution_cutoff);
    drop(input);
    let (cleanup, status) = cleanup_child(&mut child, session_id, deadline);
    let drain_deadline = (Instant::now() + FINAL_DRAIN_GRACE).min(deadline.instant());
    let drain = capture.finish_until(drain_deadline);
    let (stdout, stderr) = capture.into_strings();
    let cleanup_note = result_note(&cleanup);
    let reap_note = result_note(&status);
    let drain_note = result_note(&drain);

    match wait {
        Ok(WaitOutcome::Completed) => {
            cleanup.context(format!(
                "clean subprocess session; stdout={stdout:?}; stderr={stderr:?}"
            ))?;
            let status = status.context("reap completed subprocess")?;
            drain.context("drain completed subprocess output")?;
            Ok((status.code().unwrap_or(1), stdout, stderr))
        }
        Ok(WaitOutcome::TimedOut) => bail!(
            "subprocess timed out; session_cleanup={cleanup_note}; reap={reap_note}; \
             output_drain={drain_note}; stdout={stdout:?}; stderr={stderr:?}"
        ),
        Err(error) => Err(error).context(format!(
            "wait for subprocess; session_cleanup={cleanup_note}; reap={reap_note}; \
             output_drain={drain_note}; stdout={stdout:?}; stderr={stderr:?}"
        )),
    }
}

pub fn run_before(
    command: Command,
    deadline: Deadline,
    runtime_cap: Duration,
) -> Result<(i32, String, String)> {
    run_before_input(command, deadline, runtime_cap, None)
}

pub fn run_before_with_input(
    command: Command,
    deadline: Deadline,
    runtime_cap: Duration,
    input: &[u8],
) -> Result<(i32, String, String)> {
    run_before_input(command, deadline, runtime_cap, Some(input))
}

pub fn run_with_timeout(command: Command, timeout: Duration) -> Result<(i32, String, String)> {
    run_before(command, Deadline::after(timeout + CLEANUP_RESERVE), timeout)
}

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
