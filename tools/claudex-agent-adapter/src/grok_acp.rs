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

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::{
    agent_backend::AcpLaunch,
    app_server::{ThreadEvents, events::ThreadEventDispatcher},
};

mod client;
mod connection;
mod plugin;
mod prompt;
mod turns;
mod updates;

const COMMAND_QUEUE_CAPACITY: usize = 32;
const TURN_QUEUE_CAPACITY: usize = 8;

use connection::AcpProvider;
use turns::{cancel_turn, drive_turns, queue_turn};

#[cfg(test)]
use turns::{CancelRequest, PreparedTurn};

enum DriverCommand {
    CreateSession {
        params: Value,
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
    turn_permits: Arc<tokio::sync::Semaphore>,
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
        let turn_permits = Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY));
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
            turn_permits,
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
        TURN_QUEUE_CAPACITY
    }

    pub async fn create_session(&self, params: Value) -> Result<Value> {
        self.call(|response| DriverCommand::CreateSession { params, response })
            .await
    }

    pub async fn start_turn(&self, params: Value) -> Result<()> {
        let permit = Arc::clone(&self.turn_permits)
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("ACP driver is unavailable"))?;
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
    )
    .await;
    let Ok((connection, child)) = started else {
        let _ = setup.ready.send(started.map(|_| ()));
        setup.alive.store(false, Ordering::Relaxed);
        setup.events.close();
        return;
    };
    let _ = setup.ready.send(Ok(()));
    drive_commands(
        setup.provider,
        Rc::new(connection),
        child,
        &setup.model,
        &setup.cwd,
        &mut commands,
        &setup.events,
    )
    .await;
    setup.alive.store(false, Ordering::Relaxed);
    setup.events.close();
}

async fn drive_commands(
    provider: AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    _child: tokio::process::Child,
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
            DriverCommand::CreateSession { params, response } => {
                let result =
                    create_session(provider, &connection, model, cwd, params, &instructions).await;
                let _ = response.send(result);
            }
            DriverCommand::StartTurn {
                params,
                permit,
                response,
            } => {
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
                let _ = response.send(result);
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

async fn create_session(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    model: &str,
    cwd: &Path,
    params: Value,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<Value> {
    let response = connection
        .new_session(
            acp::NewSessionRequest::new(cwd)
                .mcp_servers(vec![])
                .meta(json!({ "modelId": model }).as_object().cloned()),
        )
        .await
        .map_err(|error| anyhow!("{} ACP session/new failed: {error:?}", provider.label()))?;
    let session_id = response.session_id.0.to_string();
    let base = prompt::provider_instructions(&params, provider == AcpProvider::Grok);
    if !base.is_empty() {
        instructions.borrow_mut().insert(session_id.clone(), base);
    }
    Ok(json!({"thread":{"id":session_id}}))
}

#[cfg(test)]
mod tests;
