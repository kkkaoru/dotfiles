use super::*;
use crate::launcher::RetainedGeneration;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn retained(path: &std::path::Path, listen: &str, sessions: &[&str]) -> RetainedProxy {
    RetainedProxy::from_path(
        path.to_path_buf(),
        RetainedGeneration {
            listen: listen.parse().unwrap(),
            pid: 1,
            build_id: "old".to_owned(),
            session_ids: sessions.iter().map(|id| (*id).to_owned()).collect(),
        },
    )
}

#[test]
fn retained_proxy_owns_only_listed_sessions() {
    let root = tempfile::tempdir().expect("retained proxy fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    assert!(proxy.owns("session-a"));
    assert!(!proxy.owns("session-b"));
    assert!(!proxy.owns(""));
}

#[test]
fn retained_proxy_reloads_listen_and_sessions_from_disk() {
    let root = tempfile::tempdir().expect("retained reload fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    assert!(proxy.owns("session-a"));
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:65108","pid":1,"build_id":"old","session_ids":["session-busy"]}"#,
    )
    .expect("update retained");
    assert!(!proxy.owns("session-a"));
    assert!(proxy.owns("session-busy"));
    assert_eq!(
        *proxy.listen.read().expect("listen"),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 65108)
    );
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
