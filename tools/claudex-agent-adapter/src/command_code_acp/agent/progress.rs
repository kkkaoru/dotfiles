use agent_client_protocol::{self as acp, Client as _};
use tokio::sync::{mpsc, oneshot};

use super::ClientOperation;
use crate::command_code_acp::events::progress_to_updates;

pub(super) async fn relay_client_operations(
    connection: acp::AgentSideConnection,
    mut requests: mpsc::UnboundedReceiver<ClientOperation>,
) {
    while let Some(request) = requests.recv().await {
        match request {
            ClientOperation::Notify(notification, sent) => {
                let _ = connection.session_notification(notification).await;
                let _ = sent.send(());
            }
        }
    }
}

pub(super) fn forward_progress_updates(
    operations: &mpsc::UnboundedSender<ClientOperation>,
    session_id: &acp::SessionId,
    event: &crate::command_code_acp::events::ProgressEvent,
) -> bool {
    for update in progress_to_updates(event) {
        // Fire-and-forget: waiting for ACP ack serializes live ▶/thinking
        // behind prompt() completion on the client.
        let (sent, _received) = oneshot::channel();
        if operations
            .send(ClientOperation::Notify(
                acp::SessionNotification::new(session_id.clone(), update),
                sent,
            ))
            .is_err()
        {
            return false;
        }
    }
    true
}

pub(super) async fn emit_progress_events(
    mut rx: mpsc::UnboundedReceiver<crate::command_code_acp::events::ProgressEvent>,
    operations: mpsc::UnboundedSender<ClientOperation>,
    notify_session: acp::SessionId,
) {
    while keep_emitting_progress(&mut rx, &operations, &notify_session).await {}
}

pub(super) async fn keep_emitting_progress(
    rx: &mut mpsc::UnboundedReceiver<crate::command_code_acp::events::ProgressEvent>,
    operations: &mpsc::UnboundedSender<ClientOperation>,
    notify_session: &acp::SessionId,
) -> bool {
    let Some(event) = rx.recv().await else {
        return false;
    };
    forward_progress_updates(operations, notify_session, &event)
}
