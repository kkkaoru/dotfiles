use super::*;
use crate::launcher::RetainedGeneration;
use std::{net::SocketAddr, path::PathBuf, sync::RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn proxy(listen: SocketAddr, pid: u32) -> RetainedProxy {
    RetainedProxy::from_path(
        PathBuf::from("/nonexistent/retained.json"),
        RetainedGeneration {
            listen,
            pid,
            build_id: "old".to_owned(),
            session_ids: vec!["session-a".to_owned()],
            agent_ids: Vec::new(),
            agent_ages: Default::default(),
        },
    )
}

fn active_health(pid: u32) -> RetainedHealthProbe {
    RetainedHealthProbe {
        status: "ok".to_owned(),
        pid: Some(pid),
        active_http_requests: 1,
        active_provider_turns: 0,
        active_subagent_models: Default::default(),
        active_subagent_agent_ids: None,
        recent_subagent_agent_ids: Default::default(),
        idle_seconds: Some(0),
        active_claude_session_ids: vec!["session-a".to_owned()],
        busy_claude_session_ids: Vec::new(),
    }
}

fn poison<T>(lock: &RwLock<T>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = lock
            .write()
            .expect("lock must be writable before poisoning");
        panic!("poison for test");
    }));
    assert!(result.is_err(), "poisoning panic must have unwound");
    assert!(lock.write().is_err(), "lock must now report poisoned");
}

#[tokio::test]
async fn should_proxy_returns_false_when_listen_state_is_poisoned() {
    let proxy = proxy("127.0.0.1:9".parse().unwrap(), 1);
    poison(&proxy.listen);

    assert!(!proxy.should_proxy_session("session-a", None).await);
}

async fn read_complete_health_request(stream: &mut tokio::net::TcpStream) {
    const MAX_REQUEST_BYTES: usize = 64 * 1024;

    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.expect("health request");
        assert!(read > 0, "health request ended before its headers");
        request.extend_from_slice(&chunk[..read]);
        assert!(
            request.len() <= MAX_REQUEST_BYTES,
            "health request exceeded fixture limit"
        );
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return;
        }
    }
}

#[tokio::test]
async fn invalid_health_body_is_transient() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("health listener");
    let listen = listener.local_addr().expect("health address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("health request");
        read_complete_health_request(&mut stream).await;
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n?";
        stream.write_all(response).await.expect("health response");
    });
    let proxy = proxy(listen, std::process::id());

    assert!(!proxy.should_proxy_session("session-a", None).await);
    task.await.expect("health task");
}

#[test]
fn decide_sticky_proxy_tolerates_poisoned_last_work_lock() {
    let proxy = proxy("127.0.0.1:9".parse().unwrap(), 1);
    poison(&proxy.last_work_at);

    assert!(proxy.decide_sticky_proxy(&active_health(1), 1, "session-a", None));
}

#[test]
fn decide_sticky_proxy_tolerates_poisoned_recent_agents_lock() {
    let proxy = proxy("127.0.0.1:9".parse().unwrap(), 1);
    poison(&proxy.recent_agents);

    assert!(proxy.decide_sticky_proxy(&active_health(1), 1, "session-a", None));
}
