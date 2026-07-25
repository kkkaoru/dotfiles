use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol as acp;
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::{
    agent_backend::AcpLaunch,
    app_server::{ThreadEvents, events::ThreadEventDispatcher},
};

mod client;
mod connection;
mod plugin;
mod prompt;
mod session;
mod turns;
mod updates;

const COMMAND_QUEUE_CAPACITY: usize = 32;
const SESSION_QUEUE_CAPACITY: usize = 1;
const TURN_QUEUE_CAPACITY: usize = 8;
const CONFIGURED_TURN_QUEUE_CAPACITY: usize = 2;
/// Reserved so SubAgent bursts cannot starve interactive user turns.
const OUTER_TURN_RESERVE: usize = 1;

use connection::AcpProvider;
use turns::{acquire_turn_permit, cancel_turn, drive_turns, queue_turn};

#[cfg(test)]
use turns::{CancelRequest, PreparedTurn};

enum DriverCommand {
    CreateSession {
        params: Value,
        _permit: tokio::sync::OwnedSemaphorePermit,
        response: oneshot::Sender<Result<Value>>,
    },
    StartTurn {
        params: Value,
        permit: tokio::sync::OwnedSemaphorePermit,
        response: oneshot::Sender<Result<()>>,
    },
    CancelTurn {
        session_id: String,
        response: oneshot::Sender<Result<()>>,
    },
}

struct DriverSetup {
    provider: AcpProvider,
    program: OsString,
    arguments: Option<Vec<String>>,
    model: String,
    cwd: PathBuf,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
    ready: oneshot::Sender<Result<()>>,
}

pub struct GrokAcp {
    commands: mpsc::Sender<DriverCommand>,
    session_permits: Arc<tokio::sync::Semaphore>,
    turn_permits: Arc<tokio::sync::Semaphore>,
    outer_permits: Arc<tokio::sync::Semaphore>,
    turn_capacity: usize,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
}

impl GrokAcp {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        let program = std::env::var_os("CLAUDEX_GROK_PROGRAM").unwrap_or_else(|| "grok".into());
        let cwd = std::env::current_dir().context("resolve Grok ACP working directory")?;
        Self::spawn_provider(AcpProvider::Grok, model, program, None, cwd).await
    }

    pub async fn spawn_copilot(model: &str) -> Result<Arc<Self>> {
        let program =
            std::env::var_os("CLAUDEX_COPILOT_PROGRAM").unwrap_or_else(|| "copilot".into());
        let cwd = std::env::current_dir().context("resolve Copilot ACP working directory")?;
        Self::spawn_provider(AcpProvider::Copilot, model, program, None, cwd).await
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(AcpProvider::Grok, model, program, None, cwd).await
    }

    pub async fn spawn_copilot_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(AcpProvider::Copilot, model, program, None, cwd).await
    }

    pub async fn spawn_configured(model: &str, launch: &AcpLaunch) -> Result<Arc<Self>> {
        let cwd = std::env::current_dir().context("resolve configured ACP working directory")?;
        Self::spawn_provider(
            AcpProvider::Configured,
            model,
            &launch.program,
            Some(launch.arguments.clone()),
            cwd,
        )
        .await
    }

    async fn spawn_provider(
        provider: AcpProvider,
        model: &str,
        program: impl Into<OsString>,
        arguments: Option<Vec<String>>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let session_permits = Arc::new(tokio::sync::Semaphore::new(SESSION_QUEUE_CAPACITY));
        let turn_capacity = match provider {
            AcpProvider::Configured => CONFIGURED_TURN_QUEUE_CAPACITY,
            AcpProvider::Grok | AcpProvider::Copilot => TURN_QUEUE_CAPACITY,
        };
        let turn_permits = Arc::new(tokio::sync::Semaphore::new(
            turn_capacity.saturating_sub(OUTER_TURN_RESERVE).max(1),
        ));
        let outer_permits = Arc::new(tokio::sync::Semaphore::new(OUTER_TURN_RESERVE));
        let events = Arc::new(ThreadEventDispatcher::default());
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = oneshot::channel();
        let driver_events = Arc::clone(&events);
        let driver_alive = Arc::clone(&alive);
        let model = model.to_owned();
        let program = program.into();
        std::thread::Builder::new()
            .name(provider.driver_name().to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build ACP runtime");
                let local = tokio::task::LocalSet::new();
                runtime.block_on(local.run_until(run_driver(
                    DriverSetup {
                        provider,
                        program,
                        arguments,
                        model,
                        cwd,
                        events: driver_events,
                        alive: driver_alive,
                        ready: ready_tx,
                    },
                    command_rx,
                )));
            })
            .with_context(|| format!("start {} ACP driver thread", provider.label()))?;
        ready_rx
            .await
            .with_context(|| format!("{} ACP driver stopped during startup", provider.label()))??;
        Ok(Arc::new(Self {
            commands: command_tx,
            session_permits,
            turn_permits,
            outer_permits,
            turn_capacity,
            events,
            alive,
        }))
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        self.events.subscribe(thread_id)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub const fn turn_capacity(&self) -> usize {
        self.turn_capacity
    }

    pub async fn create_session(&self, params: Value) -> Result<Value> {
        let permit = Arc::clone(&self.session_permits)
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("ACP driver is unavailable"))?;
        self.call(|response| DriverCommand::CreateSession {
            params,
            _permit: permit,
            response,
        })
        .await
    }

    pub async fn start_turn(&self, params: Value) -> Result<()> {
        let is_user = params.get("priority").and_then(Value::as_str) == Some("user");
        let permit = acquire_turn_permit(&self.turn_permits, &self.outer_permits, is_user).await?;
        self.call(|response| DriverCommand::StartTurn {
            params,
            permit,
            response,
        })
        .await
    }

    pub async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        self.call(|response| DriverCommand::CancelTurn {
            session_id: session_id.to_owned(),
            response,
        })
        .await
    }

    async fn call<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T>>) -> DriverCommand,
    ) -> Result<T> {
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(command(response_tx))
            .await
            .map_err(|_| anyhow!("ACP driver is unavailable"))?;
        response_rx
            .await
            .context("ACP driver dropped its response")?
    }
}

async fn run_driver(setup: DriverSetup, mut commands: mpsc::Receiver<DriverCommand>) {
    let started = connection::start(
        setup.provider,
        &setup.program,
        setup.arguments.as_deref(),
        &setup.model,
        &setup.cwd,
        Arc::clone(&setup.events),
        Arc::clone(&setup.alive),
    )
    .await;
    let Ok((connection, mut child, io_stopped, process_group)) = started else {
        let _ = setup.ready.send(started.map(|_| ()));
        setup.alive.store(false, Ordering::Relaxed);
        setup.events.close();
        return;
    };
    let _ = setup.ready.send(Ok(()));
    tokio::select! {
        () = drive_commands(
            setup.provider,
            Rc::new(connection),
            &setup.model,
            &setup.cwd,
            &mut commands,
            &setup.events,
        ) => {}
        status = child.wait() => match status {
            Ok(status) => tracing::warn!(
                provider = setup.provider.label(),
                %status,
                "ACP provider exited"
            ),
            Err(error) => tracing::error!(
                provider = setup.provider.label(),
                ?error,
                "failed to wait for ACP provider"
            ),
        },
        _ = io_stopped => tracing::warn!(
            provider = setup.provider.label(),
            "ACP provider I/O closed"
        ),
    }
    connection::terminate_process_group(process_group);
    setup.alive.store(false, Ordering::Relaxed);
    setup.events.close();
}

async fn drive_commands(
    provider: AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    model: &str,
    cwd: &Path,
    commands: &mut mpsc::Receiver<DriverCommand>,
    events: &Arc<ThreadEventDispatcher>,
) {
    let instructions = Rc::new(RefCell::new(HashMap::<String, String>::new()));
    let active_turns = Rc::new(RefCell::new(HashMap::new()));
    let invalidated_sessions = Rc::new(RefCell::new(HashSet::new()));
    let (turns, turn_receiver) = mpsc::channel(TURN_QUEUE_CAPACITY);
    let turn_worker = tokio::task::spawn_local(drive_turns(
        provider,
        Rc::clone(&connection),
        model.to_owned(),
        turn_receiver,
        Arc::clone(events),
        Rc::clone(&active_turns),
        Rc::clone(&invalidated_sessions),
    ));
    while let Some(command) = commands.recv().await {
        match command {
            DriverCommand::CreateSession {
                params,
                _permit: permit,
                response,
            } => {
                // ACP connections multiplex requests. Do not hold the driver command loop while a
                // provider creates one session: a parallel Agent burst would otherwise serialize
                // every session/new request and also strand already-created sessions' turn/start
                // commands behind the remaining session creations.
                session::Task {
                    provider,
                    connection: Rc::clone(&connection),
                    model: model.to_owned(),
                    cwd: cwd.to_owned(),
                    params,
                    instructions: Rc::clone(&instructions),
                    permit,
                    response,
                }
                .spawn();
            }
            DriverCommand::StartTurn {
                params,
                permit,
                response,
            } => {
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
                finish_start_turn(&active_turns, &session_id, response, result);
            }
            DriverCommand::CancelTurn {
                session_id,
                response,
            } => cancel_turn(&active_turns, &session_id, response),
        }
    }
    drop(turns);
    let _ = turn_worker.await;
}

fn finish_start_turn(
    active_turns: &turns::ActiveTurns,
    session_id: &str,
    response: oneshot::Sender<Result<()>>,
    result: Result<()>,
) {
    let Err(unsent) = response.send(result) else {
        return;
    };
    if unsent.is_ok() {
        let (cancelled, _result) = oneshot::channel();
        cancel_turn(active_turns, session_id, cancelled);
    }
}

#[cfg(test)]
mod tests;
