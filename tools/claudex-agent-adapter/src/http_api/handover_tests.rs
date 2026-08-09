use super::*;
use crate::launcher::RetainedGeneration;

#[test]
fn retained_proxy_owns_only_listed_sessions() {
    let proxy = RetainedProxy::from_generation(RetainedGeneration {
        listen: "127.0.0.1:9".parse().unwrap(),
        pid: 1,
        build_id: "old".to_owned(),
        session_ids: vec!["session-a".to_owned()],
    });
    assert!(proxy.owns("session-a"));
    assert!(!proxy.owns("session-b"));
    assert!(!proxy.owns(""));
}

#[test]
fn rebind_request_requires_ephemeral_or_listen() {
    let ephemeral: RebindRequest = serde_json::from_str(r#"{"ephemeral":true}"#).unwrap();
    assert!(ephemeral.ephemeral);
    assert!(ephemeral.listen.is_none());
    let bind: RebindRequest = serde_json::from_str(r#"{"listen":"127.0.0.1:8318"}"#).unwrap();
    assert!(!bind.ephemeral);
    assert_eq!(bind.listen.as_deref(), Some("127.0.0.1:8318"));
}
