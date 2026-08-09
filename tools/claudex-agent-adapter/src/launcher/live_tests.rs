use super::*;
use crate::agent_backend::{BackendKind, BackendRoute};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use super::super::{AdapterOptions, LOCAL_TOKEN, ServiceConfig};

fn config(root: &std::path::Path) -> ServiceConfig {
    ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: root.join("adapter.log"),
        lock_path: root.join("adapter.lock"),
    }
}

#[test]
fn publishes_and_reads_the_current_generation_listener() {
    let root = tempfile::tempdir().expect("live fixture");
    let config = config(root.path());
    let listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 52890);
    publish_listen(&config, listen, Some(99)).expect("publish live");
    let loaded = read(&config).expect("read live").expect("present");
    assert_eq!(loaded.listen, listen);
    assert_eq!(loaded.build_id, env!("CLAUDEX_BUILD_ID"));
    assert_eq!(loaded.pid, Some(99));
}

#[test]
fn parses_loopback_http_urls_and_rejects_invalid_live_records() {
    assert_eq!(
        parse_listen_url("http://127.0.0.1:52890/").expect("url"),
        "127.0.0.1:52890".parse().unwrap()
    );
    assert!(parse_listen_url("not-a-url").is_err());

    let root = tempfile::tempdir().expect("invalid live fixture");
    let path = root.path().join("live.8318.json");
    std::fs::write(&path, br#"{"listen":"8.8.8.8:80","build_id":"b","pid":1}"#)
        .expect("non-loopback");
    assert!(read_live(&path).is_err());
    assert!(
        read_live(&root.path().join("missing.json"))
            .expect("missing")
            .is_none()
    );
}

#[test]
fn round_trips_retained_generation_state() {
    let root = tempfile::tempdir().expect("retained fixture");
    let config = config(root.path());
    let listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54321);
    let path = write_retained(
        &config,
        listen,
        77,
        "old-build",
        vec!["session-a".to_owned(), "session-b".to_owned()],
    )
    .expect("write retained");
    let loaded = read_retained(&path).expect("read").expect("present");
    assert_eq!(loaded.pid, 77);
    assert_eq!(loaded.session_ids, ["session-a", "session-b"]);
    assert_eq!(loaded.listen, listen);
}
