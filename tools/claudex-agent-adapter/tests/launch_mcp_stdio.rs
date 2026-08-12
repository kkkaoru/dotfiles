use std::{
    io::Write as _,
    process::{Command, Stdio},
};

#[test]
fn binary_runs_the_real_launch_mcp_stdio_wrapper() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claudex-agent-adapter"))
        .arg("mcp-claudex-launch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start launch MCP stdio wrapper");
    child
        .stdin
        .take()
        .expect("launch MCP stdin")
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":21,\"method\":\"ping\"}\n")
        .expect("write launch MCP request");
    let output = child.wait_with_output().expect("wait for launch MCP");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("decode launch MCP response");
    assert_eq!(response["id"], 21);
    assert_eq!(response["result"], serde_json::json!({}));
}
