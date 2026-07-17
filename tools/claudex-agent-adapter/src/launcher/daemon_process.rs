use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

const LEGACY_ADAPTER_NAMES: &[&str] = &["claudex-app-server-adapter"];

pub(super) fn matches(pid: u32, executable: &Path) -> bool {
    fields_match(
        process_field(pid, "comm="),
        process_field(pid, "command="),
        executable,
    )
}

fn fields_match(program: Option<String>, command: Option<String>, executable: &Path) -> bool {
    let Some(program) = program else {
        return false;
    };
    let Some(command) = command else {
        return false;
    };
    command_matches(&program, &command, executable)
}

pub(super) fn terminate(pid: u32) {
    let _status = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn process_field(pid: u32, field: &str) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn command_matches(program: &str, command: &str, executable: &Path) -> bool {
    let program = Path::new(program);
    let current = program == executable;
    let renamed = program.parent() == executable.parent()
        && program.file_name().is_some_and(|name| {
            LEGACY_ADAPTER_NAMES
                .iter()
                .any(|legacy| name == OsStr::new(legacy))
        });
    let subcommand = command
        .strip_prefix(&program.to_string_lossy().to_string())
        .and_then(|arguments| arguments.split_ascii_whitespace().next());
    (current || renamed) && subcommand == Some("serve")
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn recognizes_current_and_renamed_adapter_daemons() {
        let executable = Path::new("/tmp/claudex-agent-adapter");
        assert!(command_matches(
            "/tmp/claudex-agent-adapter",
            "/tmp/claudex-agent-adapter serve --model current",
            executable
        ));
        assert!(command_matches(
            "/tmp/claudex-app-server-adapter",
            "/tmp/claudex-app-server-adapter serve --model legacy",
            executable
        ));
        assert!(!command_matches(
            "/tmp/claudex-agent-adapter",
            "/tmp/claudex-agent-adapter launch --model current",
            executable
        ));
        assert!(!command_matches(
            "/usr/local/bin/claudex-app-server-adapter",
            "/usr/local/bin/claudex-app-server-adapter serve --model legacy",
            executable
        ));
        assert!(!command_matches(
            "/tmp/unrelated-adapter",
            "/tmp/unrelated-adapter serve --model current",
            executable
        ));
        assert!(!command_matches("", "", executable));
        assert!(!matches(u32::MAX, executable));
        assert!(!fields_match(
            Some("/tmp/claudex-agent-adapter".to_owned()),
            None,
            executable
        ));
    }
}
