use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
};

use agent_client_protocol as acp;
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};

use super::{ClientOperation, HeadlessAgent};
use crate::command_code_acp::Options;

fn agent(operations: mpsc::UnboundedSender<ClientOperation>) -> HeadlessAgent {
    HeadlessAgent {
        options: Options::parse(["--cmd", "/bin/true"]).expect("options"),
        operations,
        next_session: Cell::new(0),
        session_cwds: RefCell::new(HashMap::<String, PathBuf>::new()),
        cancelled: RefCell::new(HashMap::new()),
        running: RefCell::new(HashMap::new()),
        prompt_lock: Mutex::new(()),
    }
}

fn notify_update() -> acp::SessionUpdate {
    acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new("test"))
}

fn spawn_notify(agent: Rc<HeadlessAgent>) -> JoinHandle<acp::Result<()>> {
    tokio::task::spawn_local(async move {
        agent
            .notify(
                acp::SessionId::new("notify-session".to_owned()),
                notify_update(),
            )
            .await
    })
}

async fn notification_request(
    requests: &mut mpsc::UnboundedReceiver<ClientOperation>,
) -> (acp::SessionNotification, oneshot::Sender<()>) {
    match requests.recv().await {
        Some(ClientOperation::Notify(notification, acknowledged)) => (notification, acknowledged),
        None => panic!("notify operation"),
    }
}

#[tokio::test]
async fn notify_returns_after_the_relay_acknowledges_the_update() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (operations, mut requests) = mpsc::unbounded_channel();
            let agent = Rc::new(agent(operations));
            let pending = spawn_notify(Rc::clone(&agent));
            let (notification, acknowledged) = notification_request(&mut requests).await;
            assert_eq!(notification.session_id.0.as_ref(), "notify-session");
            acknowledged.send(()).expect("acknowledge notification");
            assert!(pending.await.expect("notify task").is_ok());
        })
        .await;
}

#[tokio::test]
async fn notify_reports_a_closed_relay_channel() {
    let (operations, requests) = mpsc::unbounded_channel();
    drop(requests);
    let result = agent(operations)
        .notify(
            acp::SessionId::new("closed-session".to_owned()),
            notify_update(),
        )
        .await;
    assert!(result.is_err());
}
