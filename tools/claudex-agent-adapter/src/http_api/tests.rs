use super::*;

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
        "gpt-5.6-sol,grok-4.5".parse().unwrap(),
    );
    assert_eq!(
        subagent_policy::request_models(&headers)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        ["gpt-5.6-sol", "grok-4.5"]
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
fn distinguishes_omitted_tools_from_an_explicit_empty_array() {
    let (_, omitted) = decode_messages_request(json!({"model":"main"})).unwrap();
    let (explicit, provided) = decode_messages_request(json!({"model":"main","tools":[]})).unwrap();
    assert!(!omitted);
    assert!(provided);
    assert!(explicit.tools.is_empty());
}

#[test]
fn applies_domain_filters_to_search_urls() {
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
}
