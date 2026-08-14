use std::future::Future;

use agent_client_protocol as acp;

pub(super) fn is_unknown_session_acp_error(error: &acp::Error) -> bool {
    crate::anthropic::is_unknown_session_text(&error.to_string())
}

pub(super) async fn retry_unknown_session_once<F>(
    first: acp::Result<acp::PromptResponse>,
    saw_activity: bool,
    retry: F,
) -> (acp::Result<acp::PromptResponse>, bool)
where
    F: Future<Output = acp::Result<acp::PromptResponse>>,
{
    match first {
        Err(error) if should_retry_unknown_session(&error, saw_activity) => (retry.await, true),
        other => (other, false),
    }
}

pub(super) fn should_retry_unknown_session(error: &acp::Error, saw_activity: bool) -> bool {
    is_unknown_session_acp_error(error) && !saw_activity
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use agent_client_protocol as acp;

    use super::is_unknown_session_acp_error;

    #[test]
    fn t16_detects_configured_launch_unknown_session_details() {
        let detail = r#"ConfiguredLaunch ACP prompt failed: Internal error: {
  "details": "unknown session: 1786642532386_lDiqV_cli"
}"#;
        assert!(crate::anthropic::is_unknown_session_text(detail));
        assert!(crate::anthropic::is_unknown_session_text(
            "unknown session: 1786642532386_lDiqV_cli"
        ));
        assert!(!crate::anthropic::is_unknown_session_text(
            "ACP quota exhausted: weekly"
        ));
        assert!(!crate::anthropic::is_unknown_session_text(
            "401 unauthorized"
        ));
        assert!(!crate::anthropic::is_unknown_session_text(
            "ACP prompt timed out"
        ));
        let _ = is_unknown_session_acp_error;
    }

    async fn ok_prompt() -> acp::Result<acp::PromptResponse> {
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn still_unknown() -> acp::Result<acp::PromptResponse> {
        Err(acp::Error::internal_error().data("unknown session: still-missing"))
    }

    async fn quota_must_not_retry() -> acp::Result<acp::PromptResponse> {
        panic!("quota must not retry")
    }

    #[tokio::test]
    async fn t16_retries_unknown_session_once_and_stays_fatal_on_second() {
        let first = Err(acp::Error::internal_error()
            .data(r#"{"details":"unknown session: 1786642532386_lDiqV_cli"}"#));
        let (retried, used) = super::retry_unknown_session_once(first, false, ok_prompt()).await;
        assert!(used);
        assert!(retried.is_ok());

        let again = Err(acp::Error::internal_error().data("unknown session: still-missing"));
        let (fatal, used) = super::retry_unknown_session_once(again, false, still_unknown()).await;
        assert!(used);
        assert!(fatal.is_err());
        assert!(super::is_unknown_session_acp_error(
            fatal.as_ref().err().unwrap()
        ));

        let quota = Err(acp::Error::internal_error().data("ACP quota exhausted"));
        let (kept, used) =
            super::retry_unknown_session_once(quota, false, quota_must_not_retry()).await;
        assert!(!used);
        assert!(kept.is_err());

        let mid_turn = Err(acp::Error::internal_error().data("unknown session: after-tools"));
        let (kept, used) =
            super::retry_unknown_session_once(mid_turn, true, quota_must_not_retry()).await;
        assert!(!used);
        assert!(kept.is_err());
        assert!(!super::should_retry_unknown_session(
            kept.as_ref().err().unwrap(),
            true
        ));
    }
}
