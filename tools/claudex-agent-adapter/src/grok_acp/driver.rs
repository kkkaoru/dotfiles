use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
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
    let Ok((connection, mut child, io_stopped, process_group)) = started else {
        let _ = setup.ready.send(started.map(|_| ()));
        setup.alive.store(false, Ordering::Relaxed);
        setup.events.close();
        return;
    };
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
        ) => shutdown,
        status = child.wait() => {
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
    connection::terminate_process_group(process_group);
    if let Err(error) = child.wait().await {
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

async fn drive_commands(
    provider: super::connection::AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    model: &str,
    cwd: &Path,
    commands: &mut mpsc::Receiver<DriverCommand>,
    events: &Arc<ThreadEventDispatcher>,
    alive: &Arc<AtomicBool>,
) -> Option<oneshot::Sender<()>> {
    let instructions = Rc::new(RefCell::new(HashMap::<String, String>::new()));
    let active_turns: ActiveTurns = Rc::new(RefCell::new(HashMap::new()));
    let invalidated_sessions: InvalidatedSessions = Rc::new(RefCell::new(HashSet::new()));
    let workers = spawn_driver_workers(DriverWorkerContext {
        provider,
        connection: Rc::clone(&connection),
        model: model.to_owned(),
        events: Arc::clone(events),
        active_turns: Rc::clone(&active_turns),
        invalidated_sessions: Rc::clone(&invalidated_sessions),
        instructions: Rc::clone(&instructions),
        alive: Arc::clone(alive),
    });
    let shutdown = loop {
        let Some(command) = commands.recv().await else {
            break None;
        };
        let context = DriverCommandContext {
            provider,
            connection: &connection,
            model,
            cwd,
            instructions: &instructions,
            workers: &workers,
            active_turns: &active_turns,
        };
        if let Some(shutdown) = process_driver_command(context, command) {
            break Some(shutdown);
        }
    };
    stop_driver_workers(workers).await;
    shutdown
}

mod workers;
use workers::{process_driver_command, spawn_driver_workers, stop_driver_workers};
