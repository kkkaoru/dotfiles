//! Nonblocking pipe capture used by the command runner and process-table probe.

use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BUFFER_SIZE: usize = 8 * 1024;
const DRAIN_BUDGET: usize = 64 * 1024;
const DRAIN_ATTEMPTS: usize = 128;
const CAPTURE_POLL: Duration = Duration::from_millis(10);
const PROBE_CLEANUP_RESERVE: Duration = Duration::from_millis(50);

pub(super) struct PipeCapture {
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    stdout_bytes: Vec<u8>,
    stderr_bytes: Vec<u8>,
}

pub(super) struct CapturedCommand {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
}

pub(super) fn set_nonblocking(stream: &impl AsRawFd) -> io::Result<()> {
    let descriptor = stream.as_raw_fd();
    // SAFETY: `descriptor` belongs to the live pipe object and F_GETFL does not
    // modify Rust-owned memory.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFL updates only the kernel flags for this live descriptor.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain_stream<R: Read>(stream: &mut Option<R>, output: &mut Vec<u8>) -> io::Result<()> {
    let Some(pipe) = stream.as_mut() else {
        return Ok(());
    };
    let mut buffer = [0_u8; BUFFER_SIZE];
    let mut remaining = DRAIN_BUDGET;
    let mut attempts = DRAIN_ATTEMPTS;
    while remaining > 0 && attempts > 0 {
        attempts -= 1;
        let read_size = remaining.min(buffer.len());
        match pipe.read(&mut buffer[..read_size]) {
            Ok(0) => {
                *stream = None;
                return Ok(());
            }
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                remaining -= count;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn poll_timeout(duration: Duration) -> libc::c_int {
    duration.as_millis().max(1).min(libc::c_int::MAX as u128) as libc::c_int
}

impl PipeCapture {
    pub(super) fn from_child(child: &mut Child) -> io::Result<Self> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("subprocess stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("subprocess stderr was not piped"))?;
        set_nonblocking(&stdout)?;
        set_nonblocking(&stderr)?;
        Ok(Self {
            stdout: Some(stdout),
            stderr: Some(stderr),
            stdout_bytes: Vec::new(),
            stderr_bytes: Vec::new(),
        })
    }

    fn stdout_only(stdout: ChildStdout) -> io::Result<Self> {
        set_nonblocking(&stdout)?;
        Ok(Self {
            stdout: Some(stdout),
            stderr: None,
            stdout_bytes: Vec::new(),
            stderr_bytes: Vec::new(),
        })
    }

    pub(super) fn drain_available(&mut self) -> io::Result<()> {
        drain_stream(&mut self.stdout, &mut self.stdout_bytes)?;
        drain_stream(&mut self.stderr, &mut self.stderr_bytes)
    }

    pub(super) fn is_closed(&self) -> bool {
        self.stdout.is_none() && self.stderr.is_none()
    }

    pub(super) fn wait(&self, duration: Duration) -> io::Result<()> {
        if duration.is_zero() {
            return Ok(());
        }
        let mut descriptors = [
            libc::pollfd {
                fd: self.stdout.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
                revents: 0,
            },
            libc::pollfd {
                fd: self.stderr.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
                revents: 0,
            },
        ];
        if self.is_closed() {
            thread::sleep(duration);
            return Ok(());
        }
        // SAFETY: `descriptors` is a valid mutable array for the duration of
        // the call, and its length matches the supplied `nfds` value.
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                poll_timeout(duration),
            )
        };
        if result >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(());
        }
        Err(error)
    }

    pub(super) fn finish_until(&mut self, deadline: Instant) -> io::Result<()> {
        self.drain_available()?;
        while !self.is_closed() && Instant::now() < deadline {
            let now = Instant::now();
            self.wait(deadline.saturating_duration_since(now).min(CAPTURE_POLL))?;
            self.drain_available()?;
        }
        Ok(())
    }

    pub(super) fn into_strings(self) -> (String, String) {
        (
            String::from_utf8_lossy(&self.stdout_bytes).into_owned(),
            String::from_utf8_lossy(&self.stderr_bytes).into_owned(),
        )
    }

    fn into_stdout(self) -> Vec<u8> {
        self.stdout_bytes
    }
}

fn wait_for_capture(
    child: &mut Child,
    capture: &mut PipeCapture,
    deadline: Instant,
) -> io::Result<Option<ExitStatus>> {
    loop {
        capture.drain_available()?;
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        capture.wait(deadline.saturating_duration_since(now).min(CAPTURE_POLL))?;
    }
}

fn reap_probe_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        thread::sleep(deadline.saturating_duration_since(now).min(CAPTURE_POLL));
    }
}

fn stop_probe(child: &mut Child, deadline: Instant) {
    let _ = child.kill();
    let _ = reap_probe_until(child, deadline);
}

pub(super) fn capture_stdout_bounded(
    mut command: Command,
    deadline: Instant,
) -> io::Result<CapturedCommand> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn()?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            stop_probe(&mut child, deadline);
            return Err(io::Error::other("capture command stdout was not piped"));
        }
    };
    let mut capture = match PipeCapture::stdout_only(stdout) {
        Ok(capture) => capture,
        Err(error) => {
            stop_probe(&mut child, deadline);
            return Err(error);
        }
    };
    let execution_deadline = deadline
        .checked_sub(PROBE_CLEANUP_RESERVE)
        .unwrap_or(deadline);
    let status = match wait_for_capture(&mut child, &mut capture, execution_deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            let _ = child.kill();
            match wait_for_capture(&mut child, &mut capture, deadline)? {
                Some(status) => status,
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "capture command did not stop after SIGKILL",
                    ));
                }
            }
        }
        Err(error) => {
            stop_probe(&mut child, deadline);
            return Err(error);
        }
    };
    capture.finish_until(deadline)?;
    Ok(CapturedCommand {
        status,
        stdout: capture.into_stdout(),
    })
}
