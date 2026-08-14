use anyhow::anyhow;

use super::{ACP_SESSION_RESTART_ERROR, acp_session_restart_error};

#[test]
fn restart_failure_hides_secret_like_provider_errors_from_users() {
    let secret_like = "ACP session rejected: api_key=sk-ant-test-provider-secret";
    let error = acp_session_restart_error(
        &anyhow!(secret_like),
        &anyhow!("provider exited with status 1"),
    );
    let message = format!("{error:#}");

    assert_eq!(message, ACP_SESSION_RESTART_ERROR);
    assert!(!message.contains(secret_like));
}
