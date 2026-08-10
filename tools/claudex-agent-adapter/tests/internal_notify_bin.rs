use std::{
    fs,
    process::{Command, Stdio},
};

#[test]
fn binary_dispatches_internal_notify_before_runtime() {
    let root = tempfile::tempdir().expect("notify cache");
    let bin = env!("CARGO_BIN_EXE_claudex-agent-adapter");
    let output = Command::new(bin)
        .args([
            "__internal-notify",
            "complete",
            root.path().to_str().expect("utf8 cache"),
            "127.0.0.1:8318",
            "bin-notify-build",
        ])
        .env("CLAUDEX_MACOS_NOTIFY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run internal notify via binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let notified = fs::read_dir(root.path())
        .expect("read cache")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("hot-swap-notify")
        });
    assert!(notified, "binary notify path must write dedupe state");
}
