#[path = "support/native_claude_pty.rs"]
mod native_claude_pty;

use native_claude_pty::{AcceptanceConfig, run_acceptance};

#[test]
#[ignore = "requires explicitly opted-in, authenticated Claude Code and live SubAgent providers"]
fn real_native_agent_ui_tasks_and_prompt_responsiveness() {
    let Some(config) = AcceptanceConfig::from_environment()
        .expect("invalid native Agent PTY acceptance configuration")
    else {
        eprintln!(
            "native Agent PTY acceptance skipped; set CLAUDEX_RUN_NATIVE_AGENT_UI=1 to opt in"
        );
        return;
    };
    let evidence = run_acceptance(&config).expect("native Agent PTY acceptance failed");
    println!("{evidence}");
}
