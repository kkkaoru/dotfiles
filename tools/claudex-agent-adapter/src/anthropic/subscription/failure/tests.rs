use super::classification::{classify_failure, extract_diagnostic, status_hint};
use super::*;
use anyhow::anyhow;

#[test]
fn prefers_structured_stdout_and_redacts_diagnostics() {
    let error = process_failure_from_parts(
        "claude-test",
        "exit status: 1".to_owned(),
        br#"{"subtype":"error","is_error":true,"result":"Authentication failed api_key=fixture-secret"}"#,
        b"less useful stderr",
    );
    let failure = subscription_failure(&error).expect("typed subscription failure");

    assert_eq!(failure.kind, SubscriptionFailureKind::Authentication);
    assert!(failure.diagnostic.contains("Authentication failed"));
    assert!(!failure.diagnostic.contains("fixture-secret"));
    assert_eq!(failure.status_hint(), 401);
    assert!(!failure.is_outer_retryable());
}

#[test]
fn classifies_upstream_context_configuration_and_empty_failures() {
    let upstream = result_failure(
        Some("claude-test"),
        &serde_json::json!({"subtype":"error", "is_error":true, "status":503, "result":"Service unavailable"}),
    );
    let upstream = subscription_failure(&upstream).expect("upstream failure");
    assert_eq!(upstream.kind, SubscriptionFailureKind::UpstreamTransient);
    assert!(upstream.is_outer_retryable());
    assert!(!upstream.is_internal_retryable());
    assert_eq!(upstream.status_hint(), 503);

    let context = result_failure(
        None,
        &serde_json::json!({"subtype":"error", "result":"Prompt is too long for the context window"}),
    );
    let context = subscription_failure(&context).expect("context failure");
    assert_eq!(context.kind, SubscriptionFailureKind::ContextLimit);
    assert_eq!(context.status_hint(), 413);

    let configuration = result_failure(
        None,
        &serde_json::json!({"subtype":"error", "result":"Model not found"}),
    );
    let configuration = subscription_failure(&configuration).expect("configuration failure");
    assert_eq!(configuration.kind, SubscriptionFailureKind::Configuration);
    assert_eq!(configuration.status_hint(), 400);

    let empty = process_failure_from_parts("claude-test", "exit status: 1".to_owned(), b"", b"");
    let empty = subscription_failure(&empty).expect("empty failure");
    assert_eq!(empty.kind, SubscriptionFailureKind::EmptyProcessOutput);
    assert!(empty.is_internal_retryable());
    assert!(!empty.is_outer_retryable());
    assert_eq!(empty.status_hint(), 424);

    let stderr_upstream = process_failure_from_parts(
        "claude-test",
        "exit status: 1".to_owned(),
        b"",
        b"503 Service unavailable",
    );
    let stderr_upstream = subscription_failure(&stderr_upstream).expect("stderr failure");
    assert_eq!(
        stderr_upstream.kind,
        SubscriptionFailureKind::UpstreamTransient
    );
    assert!(stderr_upstream.is_outer_retryable());
    assert!(!stderr_upstream.is_internal_retryable());
    assert_eq!(stderr_upstream.status_hint(), 503);
}

#[test]
fn caps_diagnostics_without_splitting_utf8() {
    let diagnostic = format!("{} secret", "\u{3042}".repeat(MAX_DIAGNOSTIC_CHARS + 10));
    let error = protocol_failure(None, &diagnostic);
    let failure = subscription_failure(&error).expect("protocol failure");

    assert!(failure.diagnostic.ends_with("..."));
    assert_eq!(
        failure.diagnostic.trim_end_matches("...").chars().count(),
        MAX_DIAGNOSTIC_CHARS
    );
}

#[test]
fn redacts_case_insensitive_compact_authorization_and_cookie_values() {
    for diagnostic in [
        "Authorization: Bearer fixture-token Cookie: fixture-cookie trailing detail",
        "Authorization:Bearer fixture-token trailing detail",
        "authorization=Bearer fixture-token trailing detail",
        "AUTHORIZATION:BEARER fixture-token trailing detail",
        "Bearer: fixture-token trailing detail",
        "Bearer= fixture-token trailing detail",
        "Set-Cookie: fixture-cookie trailing detail",
        "X-Api-Key: fixture-token trailing detail",
    ] {
        let error = protocol_failure(None, diagnostic);
        let failure = subscription_failure(&error).expect("protocol failure");
        assert!(
            !failure.diagnostic.contains("fixture-token"),
            "{diagnostic}"
        );
        assert!(
            !failure.diagnostic.contains("fixture-cookie"),
            "{diagnostic}"
        );
    }
}

#[test]
fn sanitizes_control_characters_and_non_prefixed_sensitive_tokens() {
    let error = protocol_failure(
        None,
        "plain\ntext sk-test-token trailing api_key: fixture-value",
    );
    let failure = subscription_failure(&error).expect("protocol failure");
    assert!(failure.diagnostic.contains("plain text"));
    assert!(!failure.diagnostic.contains("sk-test-token"));
    assert!(!failure.diagnostic.contains("fixture-value"));
}

#[test]
fn timeout_is_typed_failed_dependency_without_any_retry_scope() {
    let error = timeout_failure("claude-test", Duration::from_secs(5));
    let failure = subscription_failure(&error).expect("typed timeout failure");

    assert_eq!(failure.kind, SubscriptionFailureKind::LocalTimeout);
    assert_eq!(failure.status_hint(), 424);
    assert!(!failure.is_internal_retryable());
    assert!(!failure.is_outer_retryable());
}

#[test]
fn numeric_substrings_do_not_misclassify_local_process_failures() {
    let error = process_failure_from_parts(
        "claude-test",
        "exit status: 1".to_owned(),
        b"",
        b"local worker pid 15003 exited",
    );
    let failure = subscription_failure(&error).expect("typed process failure");

    assert_eq!(failure.kind, SubscriptionFailureKind::LocalProcess);
    assert_eq!(failure.status_hint(), 424);
}

#[test]
fn nonzero_exit_with_success_stdout_uses_the_failure_from_stderr() {
    let error = process_failure_from_parts(
        "claude-test",
        "exit status: 7".to_owned(),
        br#"{"subtype":"success","is_error":false,"result":"completed text"}"#,
        b"Authentication failed",
    );
    let failure = subscription_failure(&error).expect("typed process failure");

    assert_eq!(failure.kind, SubscriptionFailureKind::Authentication);
    assert_eq!(failure.status_hint(), 401);
    assert!(failure.diagnostic.contains("Authentication failed"));
    assert!(!failure.diagnostic.contains("completed text"));
}

#[test]
fn renders_non_string_structured_output_and_falls_back_from_null() {
    assert_eq!(
        subscription_result_text(&serde_json::json!({
            "structured_output":{"answer":"ok"}
        })),
        Some(r#"{"answer":"ok"}"#.to_owned())
    );
    assert_eq!(
        subscription_result_text(&serde_json::json!({
            "structured_output":null,
            "result":"fallback"
        })),
        Some("fallback".to_owned())
    );
}

#[test]
fn classifies_status_and_marker_variants_without_guessing_local_errors() {
    for (value, diagnostic, expected) in [
        (
            serde_json::json!({"status":403}),
            "",
            SubscriptionFailureKind::Authentication,
        ),
        (
            serde_json::json!({"status":429}),
            "",
            SubscriptionFailureKind::UpstreamTransient,
        ),
        (
            serde_json::json!({"status":404}),
            "",
            SubscriptionFailureKind::Configuration,
        ),
        (
            serde_json::json!({"status":413}),
            "",
            SubscriptionFailureKind::ContextLimit,
        ),
        (
            serde_json::json!({}),
            "provider sakana not found",
            SubscriptionFailureKind::Configuration,
        ),
        (
            serde_json::json!({}),
            "protocol framing failed",
            SubscriptionFailureKind::Protocol,
        ),
    ] {
        assert_eq!(classify_failure(Some(&value), diagnostic), expected);
    }
    assert_eq!(
        status_hint(
            SubscriptionFailureKind::Authentication,
            Some(&serde_json::json!({"status":403})),
            None,
        ),
        403
    );
    assert_eq!(
        status_hint(SubscriptionFailureKind::UpstreamTransient, None, None),
        502
    );
    assert_eq!(
        extract_diagnostic(&serde_json::json!({"error":{"code":"E1"}})).as_deref(),
        Some("\"E1\"")
    );
    assert_eq!(
        extract_diagnostic(&serde_json::json!({"status":500})).as_deref(),
        Some("500")
    );
}

#[test]
fn formats_all_failure_kinds_and_marks_errors_after_stream_output() {
    for kind in [
        SubscriptionFailureKind::UpstreamTransient,
        SubscriptionFailureKind::Authentication,
        SubscriptionFailureKind::Configuration,
        SubscriptionFailureKind::ContextLimit,
        SubscriptionFailureKind::LocalProcess,
        SubscriptionFailureKind::LocalTimeout,
        SubscriptionFailureKind::EmptyProcessOutput,
        SubscriptionFailureKind::Protocol,
    ] {
        let failure = SubscriptionFailure::new(
            kind,
            Some("claude-test"),
            Some("exit status: 1".to_owned()),
            "diagnostic",
            424,
        );
        let rendered = failure.to_string();
        assert!(rendered.contains(kind.label()));
        assert!(rendered.contains("claude-test"));
        assert!(rendered.contains("exit status: 1"));
    }

    let typed = after_stream_output("claude-test", protocol_failure(Some("claude-test"), "oops"));
    let typed = subscription_failure(&typed).expect("typed post-stream failure");
    assert!(!typed.is_internal_retryable());
    assert!(typed.to_string().contains("stream already emitted frames"));

    let generic = after_stream_output("claude-test", anyhow!("boom"));
    let generic = subscription_failure(&generic).expect("wrapped post-stream failure");
    assert!(!generic.is_internal_retryable());
    assert!(
        generic
            .to_string()
            .contains("stream failed after emitting frames")
    );
}
