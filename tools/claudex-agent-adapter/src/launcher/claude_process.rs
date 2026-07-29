use std::{
    io::IsTerminal,
    process::{Child, ChildStderr, Command, ExitStatus},
};

pub(super) struct ClaudeProcess {
    child: Child,
    isolated: bool,
}

impl ClaudeProcess {
    pub(super) fn new(child: Child, isolated: bool) -> Self {
        Self { child, isolated }
    }

    pub(super) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub(super) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
}

/// Isolate only non-interactive children so an interactive Claude process stays
/// in the terminal foreground process group and can read from the TTY.
pub(super) fn configure(command: &mut Command) -> bool {
    let isolated = should_isolate(std::io::stdin().is_terminal());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        if isolated {
            command.process_group(0);
        }
    }
    #[cfg(not(unix))]
    {
        let _command = command;
    }
    isolated
}

fn should_isolate(stdin_is_terminal: bool) -> bool {
    !stdin_is_terminal
}

impl Drop for ClaudeProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        terminate(&mut self.child, self.isolated);
        let _status = self.child.wait();
    }
}

fn terminate(child: &mut Child, isolated: bool) {
    #[cfg(unix)]
    if isolated {
        let _result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        return;
    }
    let _result = child.kill();
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    #[test]
    fn drop_leaves_a_reaped_claude_process_alone() {
        let mut command = Command::new("true");
        let isolated = configure(&mut command);
        let mut process =
            ClaudeProcess::new(command.spawn().expect("start Claude fixture"), isolated);
        assert!(process.wait().expect("reap Claude fixture").success());
        drop(process);
    }

    #[cfg(unix)]
    #[test]
    fn drop_terminates_and_reaps_the_claude_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60 & wait"]);
        command.process_group(0);
        let child = command.spawn().expect("start Claude fixture");
        let process_group = child.id();
        drop(ClaudeProcess::new(child, true));
        let group_exists = Command::new("kill")
            .args(["-0", &format!("-{process_group}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("inspect Claude fixture group")
            .success();
        assert!(!group_exists);
    }

    #[test]
    fn interactive_children_remain_in_the_launchers_process_group() {
        assert!(!should_isolate(true));
        assert!(should_isolate(false));
    }

    #[test]
    fn drop_terminates_an_unisolated_claude_process() {
        let child = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("start Claude fixture");
        let pid = child.id();
        drop(ClaudeProcess::new(child, false));
        assert!(
            !Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("inspect Claude fixture")
                .success()
        );
    }
}
