use super::*;
use crate::{
    app_server::events::ThreadEventDispatcher,
    grok_acp::{
        client::AcpClient,
        turns::{ActiveTurns, InvalidatedSessions},
    },
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    task::LocalSet,
};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

#[tokio::test]
async fn timeout_and_quota_messages_identify_the_configured_provider() {
    let events = ThreadEventDispatcher::default();
    let active = ActiveTurns::default();
    let invalidated = InvalidatedSessions::default();
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let (_sender, mut cancellation) = tokio::sync::oneshot::channel();
    let mut permit = Some(permits.acquire_owned().await.unwrap());
    let ctl = super::TurnCtl {
        provider: crate::grok_acp::connection::AcpProvider::Configured,
        session_id: "session",
        cancellation: &mut cancellation,
        permit: &mut permit,
        events: &events,
        active_turns: &active,
        invalidated_sessions: &invalidated,
    };
    let guard = PromptGuard {
        timeout: std::time::Duration::from_secs(3),
        alive: &std::sync::atomic::AtomicBool::new(true),
        cooldown: &std::sync::atomic::AtomicBool::new(false),
        quota: None,
    };
    assert_eq!(
        timeout_message(&ctl, &guard),
        "Configured ACP prompt had no event for 3s; provider/model cooling down"
    );
    assert_eq!(
        quota_message(&ctl, "weekly limit"),
        "Configured ACP quota exhausted: weekly limit; provider/model cooling down"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn configured_quota_invalidates_the_turn_and_sends_one_cancel_notification() {
    LocalSet::new()
        .run_until(async {
            let events = std::sync::Arc::new(ThreadEventDispatcher::default());
            let receiver = events.subscribe("session");
            let active = ActiveTurns::default();
            active.borrow_mut().insert("session".to_owned(), None);
            let invalidated = InvalidatedSessions::default();
            let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let (_sender, mut cancellation) = tokio::sync::oneshot::channel();
            let mut permit = Some(permits.acquire_owned().await.unwrap());
            let (connection, requests) =
                scripted_connection(std::sync::Arc::clone(&events), vec![None, None]);
            let (quota_tx, quota_rx) = crate::grok_acp::stderr_quota::watch_channel();
            quota_tx
                .send(Some("Weekly usage limit reached".to_owned()))
                .expect("quota");
            let alive = std::sync::atomic::AtomicBool::new(true);
            let cooldown = std::sync::atomic::AtomicBool::new(false);
            run_prompt(
                super::TurnCtl {
                    provider: crate::grok_acp::connection::AcpProvider::Configured,
                    session_id: "session",
                    cancellation: &mut cancellation,
                    permit: &mut permit,
                    events: &events,
                    active_turns: &active,
                    invalidated_sessions: &invalidated,
                },
                std::rc::Rc::new(connection),
                acp::SessionId::new("session".to_owned()),
                "prompt".to_owned(),
                PromptGuard {
                    timeout: std::time::Duration::from_secs(1),
                    alive: &alive,
                    cooldown: &cooldown,
                    quota: Some(quota_rx),
                },
            )
            .await;
            let requests = requests.await.expect("request trace");
            assert_eq!(requests[0]["method"], "session/prompt");
            assert_eq!(requests[1]["method"], "session/cancel");
            assert!(invalidated.borrow().contains("session"));
            assert!(cooldown.load(std::sync::atomic::Ordering::Acquire));
            assert_eq!(receiver.recv().await.unwrap()["method"], "error");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_session_before_activity_recreates_once_and_completes_the_turn() {
    LocalSet::new()
        .run_until(async {
            let events = std::sync::Arc::new(ThreadEventDispatcher::default());
            let receiver = events.subscribe("session");
            let active = ActiveTurns::default();
            active.borrow_mut().insert("session".to_owned(), None);
            let invalidated = InvalidatedSessions::default();
            let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let (_sender, mut cancellation) = tokio::sync::oneshot::channel();
            let mut permit = Some(permits.acquire_owned().await.unwrap());
            let (connection, requests) = scripted_connection(
                std::sync::Arc::clone(&events),
                vec![
                    Some(json!({"error":{"code":-32603,"message":"unknown session: stale"}})),
                    Some(json!({"result":{"sessionId":"recreated"}})),
                    Some(json!({"result":{"stopReason":"end_turn"}})),
                ],
            );
            let alive = std::sync::atomic::AtomicBool::new(true);
            let cooldown = std::sync::atomic::AtomicBool::new(false);
            run_prompt(
                super::TurnCtl {
                    provider: crate::grok_acp::connection::AcpProvider::Grok,
                    session_id: "session",
                    cancellation: &mut cancellation,
                    permit: &mut permit,
                    events: &events,
                    active_turns: &active,
                    invalidated_sessions: &invalidated,
                },
                std::rc::Rc::new(connection),
                acp::SessionId::new("session".to_owned()),
                "prompt".to_owned(),
                PromptGuard {
                    timeout: std::time::Duration::from_secs(1),
                    alive: &alive,
                    cooldown: &cooldown,
                    quota: None,
                },
            )
            .await;
            let requests = requests.await.expect("request trace");
            assert_eq!(
                requests
                    .iter()
                    .map(|request| request["method"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["session/prompt", "session/new", "session/prompt"]
            );
            assert_eq!(
                receiver.recv().await.unwrap()["params"]["turn"]["status"],
                "completed"
            );
            assert!(!invalidated.borrow().contains("session"));
        })
        .await;
}

fn scripted_connection(
    events: std::sync::Arc<ThreadEventDispatcher>,
    replies: Vec<Option<Value>>,
) -> (
    acp::ClientSideConnection,
    tokio::sync::oneshot::Receiver<Vec<Value>>,
) {
    let (outgoing, outgoing_peer) = tokio::io::duplex(4096);
    let (incoming, incoming_peer) = tokio::io::duplex(4096);
    let (connection, io) = acp::ClientSideConnection::new(
        AcpClient::new(events),
        outgoing.compat_write(),
        incoming.compat(),
        |task| drop(tokio::task::spawn_local(task)),
    );
    drop(tokio::task::spawn_local(async move {
        let _ = io.await;
    }));
    let (trace_tx, trace_rx) = tokio::sync::oneshot::channel();
    drop(tokio::task::spawn_local(async move {
        let mut reader = BufReader::new(outgoing_peer);
        let mut incoming_peer = incoming_peer;
        let mut trace = Vec::new();
        for reply in replies {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("ACP request");
            let request: Value = serde_json::from_str(&line).expect("valid ACP request");
            write_scripted_reply(&mut incoming_peer, &request, reply).await;
            trace.push(request);
        }
        trace_tx.send(trace).expect("trace receiver");
    }));
    (connection, trace_rx)
}

async fn write_scripted_reply(
    incoming_peer: &mut tokio::io::DuplexStream,
    request: &Value,
    reply: Option<Value>,
) {
    let Some(mut reply) = reply else {
        return;
    };
    reply["jsonrpc"] = json!("2.0");
    reply["id"] = request["id"].clone();
    incoming_peer
        .write_all(reply.to_string().as_bytes())
        .await
        .expect("ACP response");
    incoming_peer
        .write_all(b"\n")
        .await
        .expect("response newline");
}
