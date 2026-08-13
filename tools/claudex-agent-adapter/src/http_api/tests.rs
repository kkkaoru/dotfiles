use super::*;

#[test]
fn prefixes_non_anthropic_models_for_gateway_discovery() {
    assert_eq!(
        crate::discovery_model_id("gpt-5.6-luna"),
        "claude-claudex-gpt-5.6-luna"
    );
    assert_eq!(
        crate::discovery_model_id("gpt-5.6-terra"),
        "claude-claudex-gpt-5.6-terra"
    );
    assert_eq!(
        crate::discovery_model_id("opencode-go/deepseek-v4-flash"),
        "claude-claudex-opencode-go/deepseek-v4-flash"
    );
    assert_eq!(
        crate::discovery_model_id("claude-sonnet-5"),
        "claude-sonnet-5"
    );
    assert_eq!(
        crate::discovery_model_id("claude-haiku-4-5"),
        "claude-haiku-4-5"
    );
}

#[test]
fn accepts_only_valid_existing_working_directory_headers() {
    let root = tempfile::tempdir().expect("working directory fixture");
    let canonical = root.path().canonicalize().expect("canonical fixture");
    let mut headers = HeaderMap::new();
    headers.insert(
        working_directory::HEADER_NAME,
        working_directory::encode(root.path())
            .parse()
            .expect("header"),
    );
    assert_eq!(request_working_directory(&headers), Some(canonical));

    headers.insert(
        working_directory::HEADER_NAME,
        "/definitely/missing".parse().expect("missing header"),
    );
    assert!(request_working_directory(&headers).is_none());
    assert!(request_working_directory(&HeaderMap::new()).is_none());
}

#[test]
fn parses_terminal_subagent_policy_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        subagent_policy::HEADER_NAME,
        "gpt-5.6-sol,grok-4.6".parse().unwrap(),
    );
    assert_eq!(
        subagent_policy::request_models(&headers)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["gpt-5.6-sol", "grok-4.6"]
    );
}

#[test]
fn parses_native_claude_code_request_identity_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        CLAUDE_CODE_SESSION_ID_HEADER,
        " session-1 ".parse().unwrap(),
    );
    headers.insert(CLAUDE_CODE_AGENT_ID_HEADER, "agent-2".parse().unwrap());
    headers.insert(
        CLAUDE_CODE_PARENT_AGENT_ID_HEADER,
        "agent-parent".parse().unwrap(),
    );

    let identity = request_identity(&headers).expect("valid identity headers");
    assert_eq!(identity.session_id(), Some("session-1"));
    assert_eq!(identity.agent_id(), Some("agent-2"));
    assert_eq!(identity.parent_agent_id(), Some("agent-parent"));
}

#[test]
fn rejects_empty_or_oversized_identity_headers() {
    let mut empty = HeaderMap::new();
    empty.insert(CLAUDE_CODE_SESSION_ID_HEADER, "   ".parse().unwrap());
    assert!(request_identity(&empty).is_err());

    let mut oversized = HeaderMap::new();
    oversized.insert(
        CLAUDE_CODE_AGENT_ID_HEADER,
        "a".repeat(MAX_CLAUDE_CODE_ID_BYTES + 1).parse().unwrap(),
    );
    assert!(request_identity(&oversized).is_err());
}

#[test]
fn derives_ccr_session_identity_from_search_path() {
    assert_eq!(
        super::logging::path_session_id("/v1/code/sessions/session_search/worker/web-search"),
        Some("session_search")
    );
    assert_eq!(
        super::logging::path_session_id("/v1/code/sessions/session_search/worker/web-search/extra"),
        None
    );
    assert_eq!(
        super::logging::path_session_id("/v1/code/sessions//worker/web-search"),
        None
    );
}

#[test]
fn distinguishes_omitted_tools_from_an_explicit_empty_array() {
    let (_, omitted) = decode_messages_request(json!({"model":"main"})).unwrap();
    let (explicit, provided) = decode_messages_request(json!({"model":"main","tools":[]})).unwrap();
    assert!(!omitted);
    assert!(provided);
    assert!(explicit.tools.is_empty());
}

#[test]
fn applies_domain_filters_to_search_urls() {
    use super::web_search::domain_allowed;
    let allowed = vec!["example.com".to_owned()];
    let blocked = vec!["blocked.example.com".to_owned()];
    assert!(domain_allowed("https://example.com/a", &allowed, &[]));
    assert!(domain_allowed("https://sub.example.com/a", &allowed, &[]));
    assert!(!domain_allowed("https://other.com/a", &allowed, &[]));
    assert!(!domain_allowed(
        "https://blocked.example.com/a",
        &[],
        &blocked
    ));
    assert!(!domain_allowed("ftp://example.com/path", &[], &[]));
    assert!(!domain_allowed("https:///path", &[], &[]));
    assert!(!domain_allowed(
        "https://example.com/path",
        &["other.com".to_string()],
        &[]
    ));
    assert!(domain_allowed(
        "HTTPS://EXAMPLE.COM/path",
        &[".EXAMPLE.COM".to_string()],
        &[]
    ));
    assert!(!domain_allowed("not-a-url", &[], &[]));
    // Empty allow-list means any https host is allowed (unless blocked).
    assert!(domain_allowed("https://anywhere.example/a", &[], &[]));
    assert!(!domain_allowed(
        "https://blocked.example.com/a",
        &[],
        &blocked
    ));
}

#[tokio::test]
async fn trace_http_request_covers_identity_and_body_error_paths() {
    use std::time::Duration;

    use axum::http::HeaderValue;
    use tokio::net::TcpListener;

    let app = Router::new()
        .route("/ok", get(|| async { "ok" }))
        .route(
            "/err",
            get(|| async {
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
                tx.try_send(Err(std::io::Error::other("boom")))
                    .expect("enqueue body error");
                drop(tx);
                Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
            }),
        )
        .route(
            "/slow",
            get(|| async {
                let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
                std::mem::forget(tx);
                Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
            }),
        )
        .layer(middleware::from_fn(logging::trace_http_request));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("logging listener");
    let addr = listener.local_addr().expect("logging address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let client = reqwest::Client::new();
    let invalid = HeaderValue::from_bytes(&[0xff]).expect("invalid utf8 header");
    let ok = client
        .get(format!("http://{addr}/ok"))
        .header(CLAUDE_CODE_SESSION_ID_HEADER, invalid)
        .send()
        .await
        .expect("ok request");
    assert_eq!(ok.text().await.expect("ok body"), "ok");
    let err = client.get(format!("http://{addr}/err")).send().await;
    if let Ok(response) = err {
        let _ = response.bytes().await;
    }
    if let Ok(slow) = client.get(format!("http://{addr}/slow")).send().await {
        drop(slow);
    }
}
