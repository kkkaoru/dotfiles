use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};

use agent_client_protocol as acp;
use tokio::sync::{mpsc, oneshot};

use super::workers::{process_driver_command, spawn_driver_workers, stop_driver_workers};
use super::{
    ActiveTurns, DriverCommand, DriverCommandContext, DriverWorkerContext, InvalidatedSessions,
    connection,
};
use crate::app_server::events::ThreadEventDispatcher;

pub(super) async fn drive_commands(
    provider: connection::AcpProvider,
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
