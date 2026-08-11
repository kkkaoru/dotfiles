use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use super::{FallbackState, read_state, reserve_listener, state_path, write_state};

#[test]
fn reserves_a_loopback_listener_for_wildcard_configuration() {
    let listen = reserve_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8318))
        .expect("fallback listener");
    assert!(listen.ip().is_loopback());
    assert_ne!(listen.port(), 0);
    let v6 = reserve_listener(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8318))
        .expect("ipv6 fallback listener");
    assert!(v6.ip().is_loopback());
}

#[test]
fn fallback_state_keeps_the_generation_identity_and_port() {
    let state = FallbackState {
        listen: "127.0.0.1:8324".parse().unwrap(),
        build_id: "build".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        pid: 42,
    };
    let encoded = serde_json::to_string(&state).unwrap();
    let decoded: FallbackState = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.listen, state.listen);
    assert_eq!(decoded.build_id, state.build_id);
    assert_eq!(
        decoded.service_config_fingerprint,
        state.service_config_fingerprint
    );
    assert_eq!(decoded.pid, state.pid);
}

#[test]
fn read_state_rejects_invalid_records_and_round_trips_valid_ones() {
    let root = tempfile::tempdir().expect("fallback state fixture");
    let path = root.path().join("fallback.8318.json");
    assert!(read_state(&path).expect("missing state").is_none());

    write_state(
        &path,
        &FallbackState {
            listen: "127.0.0.1:8325".parse().unwrap(),
            build_id: "build".to_owned(),
            service_config_fingerprint: "service".to_owned(),
            pid: 99,
        },
    )
    .expect("write valid state");
    let loaded = read_state(&path).expect("read valid").expect("present");
    assert_eq!(loaded.pid, 99);

    std::fs::write(
        &path,
        br#"{"listen":"8.8.8.8:80","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
    )
    .expect("non-loopback");
    assert!(read_state(&path).is_err());
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:0","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
    )
    .expect("port zero");
    assert!(read_state(&path).is_err());
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:80","build_id":"b","service_config_fingerprint":"s","pid":0}"#,
    )
    .expect("pid zero");
    assert!(read_state(&path).is_err());
}

#[test]
fn state_path_uses_listen_port_beside_the_adapter_log() {
    let mut config = super::super::ServiceConfig {
        options: super::super::AdapterOptions {
            routes: vec![crate::agent_backend::BackendRoute::new(
                "test-model",
                crate::agent_backend::BackendKind::CodexAppServer,
            )],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: super::super::LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "test-fingerprint".to_owned(),
        service_config_fingerprint: "service-fingerprint".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: PathBuf::from("/tmp/claudex/adapter.log"),
        lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
    };
    let path = state_path(&config).expect("state path");
    assert!(path.ends_with("fallback.8318.json"));
    config.log_path = PathBuf::new();
    assert!(state_path(&config).is_err());
}

#[test]
fn read_state_validates_port_and_ip() {
    let root = tempfile::tempdir().expect("validate port/ip fixture");
    let path = root.path().join("fallback.invalid.json");

    // Test port == 0 rejection
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:0","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
    )
    .expect("write port=0");
    assert!(read_state(&path).is_err());

    // Test non-loopback IP rejection
    std::fs::write(
        &path,
        br#"{"listen":"192.168.1.1:8000","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
    )
    .expect("write non-loopback");
    assert!(read_state(&path).is_err());
}

#[test]
fn reserves_ipv6_loopback_listener() {
    let listen_v6 = reserve_listener(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 9000))
        .expect("ipv6 fallback listener");
    assert!(listen_v6.ip().is_loopback());
    assert!(matches!(listen_v6.ip(), IpAddr::V6(_)));
}
