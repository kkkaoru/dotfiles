use super::path_session_id;

#[test]
fn extracts_only_the_expected_ccr_session_path_shape() {
    assert_eq!(
        path_session_id("/v1/code/sessions/session-123/worker/web-search"),
        Some("session-123")
    );
    assert_eq!(
        path_session_id("/v1/code/sessions/session-123/worker/web-search/extra"),
        None
    );
    assert_eq!(
        path_session_id("/v1/code/sessions/a/b/worker/web-search"),
        None
    );
    assert_eq!(path_session_id("/health"), None);
    assert_eq!(
        path_session_id("/v1/code/sessions//worker/web-search"),
        None,
        "empty session id segment must be rejected"
    );
}
