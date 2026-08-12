use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol as acp;
use anyhow::anyhow;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::{
    DriverCommand, DriverSetup, connection, queue,
    turns::{ActiveTurns, InvalidatedSessions, PreparedTurn, queue_turn},
};
use crate::app_server::events::ThreadEventDispatcher;

pub(super) struct StartTurnRequest {
    pub(super) params: Value,
    pub(super) permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) response: oneshot::Sender<anyhow::Result<()>>,
}

struct DriverWorkers {
    start_turns: mpsc::UnboundedSender<StartTurnRequest>,
    turn_worker: tokio::task::JoinHandle<()>,
    start_turn_worker: tokio::task::JoinHandle<()>,
    turns: mpsc::Sender<PreparedTurn>,
}

struct DriverWorkerContext {
    provider: super::connection::AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    model: String,
    events: Arc<ThreadEventDispatcher>,
    active_turns: ActiveTurns,
    invalidated_sessions: InvalidatedSessions,
    instructions: Rc<RefCell<HashMap<String, String>>>,
    alive: Arc<AtomicBool>,
    cooldown: Arc<AtomicBool>,
}

struct DriverCommandContext<'a> {
    provider: super::connection::AcpProvider,
    connection: &'a Rc<acp::ClientSideConnection>,
    model: &'a str,
    cwd: &'a Path,
    instructions: &'a Rc<RefCell<HashMap<String, String>>>,
    workers: &'a DriverWorkers,
    active_turns: &'a ActiveTurns,
}

pub(super) fn schedule_start_turn(
    turns: &mpsc::UnboundedSender<StartTurnRequest>,
    request: StartTurnRequest,
) {
    if let Err(request) = turns.send(request) {
        let _ = request
            .0
            .response
            .send(Err(anyhow!("ACP turn scheduler is unavailable")));
    }
}

pub(super) async fn drive_start_turns(
    provider: super::connection::AcpProvider,
    mut requests: mpsc::UnboundedReceiver<StartTurnRequest>,
    instructions: Rc<RefCell<HashMap<String, String>>>,
    turns: mpsc::Sender<PreparedTurn>,
    active_turns: ActiveTurns,
    invalidated_sessions: InvalidatedSessions,
) {
    while let Some(StartTurnRequest {
        params,
        permit,
        response,
    }) = requests.recv().await
    {
        let session_id = params
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let result = queue_turn(
            provider,
            params,
            permit,
            &instructions,
            &turns,
            &active_turns,
            &invalidated_sessions,
        )
        .await;
        queue::finish_start_turn(&active_turns, &session_id, response, result);
    }
}

pub(super) async fn run_driver(setup: DriverSetup, mut commands: mpsc::Receiver<DriverCommand>) {
    let started = connection::start(connection::StartConnection {
        provider: setup.provider,
        program: &setup.program,
        arguments: setup.arguments.as_deref(),
        model: &setup.model,
        effort: setup.effort.as_deref(),
        cwd: &setup.cwd,
        events: Arc::clone(&setup.events),
        alive: Arc::clone(&setup.alive),
    })
    .await;
    let Ok((connection, child, io_stopped, process_group)) = started else {
        let _ = setup.ready.send(started.map(|_| ()));
        setup.alive.store(false, Ordering::Relaxed);
        setup.events.close();
        return;
    };
    let mut child = connection::ProviderChild::new(child, process_group);
    let _ = setup.ready.send(Ok(()));
    let shutdown = tokio::select! {
        shutdown = drive_commands(
            setup.provider,
            Rc::new(connection),
            &setup.model,
            &setup.cwd,
            &mut commands,
            &setup.events,
            &setup.alive,
            &setup.cooldown,
        ) => shutdown,
        status = child.child.wait() => {
            match status {
                Ok(status) => tracing::warn!(provider = setup.provider.label(), %status, "ACP provider exited"),
                Err(error) => tracing::error!(provider = setup.provider.label(), ?error, "failed to wait for ACP provider"),
            }
            None
        }
        _ = io_stopped => {
            tracing::warn!(provider = setup.provider.label(), "ACP provider I/O closed");
            None
        },
    };
    if let Err(error) = child.terminate_and_wait().await {
        tracing::error!(
            provider = setup.provider.label(),
            ?error,
            "failed to reap ACP provider after termination"
        );
    }
    setup.alive.store(false, Ordering::Relaxed);
    setup.events.close();
    if let Some(response) = shutdown {
        let _ = response.send(());
    }
}

mod drive_commands;
mod workers;
use drive_commands::drive_commands;
