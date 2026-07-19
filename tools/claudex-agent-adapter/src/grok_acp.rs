use std::{
    cell::RefCell,
    collections::HashMap,
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
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::app_server::{ThreadEvents, events::ThreadEventDispatcher};

mod client;
mod connection;
mod plugin;
mod prompt;
mod updates;

const COMMAND_QUEUE_CAPACITY: usize = 32;
const TURN_QUEUE_CAPACITY: usize = 8;

use connection::AcpProvider;

enum DriverCommand {
    CreateSession {
        params: Value,
        response: oneshot::Sender<Result<Value>>,
    },
    StartTurn {
        params: Value,
        response: oneshot::Sender<Result<()>>,
    },
}

struct DriverSetup {
    provider: AcpProvider,
    program: OsString,
    model: String,
    cwd: PathBuf,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
    ready: oneshot::Sender<Result<()>>,
}

pub struct GrokAcp {
    commands: mpsc::Sender<DriverCommand>,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
}

impl GrokAcp {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        let program = std::env::var_os("CLAUDEX_GROK_PROGRAM").unwrap_or_else(|| "grok".into());
        let cwd = std::env::current_dir().context("resolve Grok ACP working directory")?;
        Self::spawn_provider(AcpProvider::Grok, model, program, cwd).await
    }

    pub async fn spawn_copilot(model: &str) -> Result<Arc<Self>> {
        let program =
            std::env::var_os("CLAUDEX_COPILOT_PROGRAM").unwrap_or_else(|| "copilot".into());
        let cwd = std::env::current_dir().context("resolve Copilot ACP working directory")?;
        Self::spawn_provider(AcpProvider::Copilot, model, program, cwd).await
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(AcpProvider::Grok, model, program, cwd).await
    }

    pub async fn spawn_copilot_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(AcpProvider::Copilot, model, program, cwd).await
    }

    async fn spawn_provider(
        provider: AcpProvider,
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
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

    pub async fn create_session(&self, params: Value) -> Result<Value> {
        self.call(|response| DriverCommand::CreateSession { params, response })
            .await
    }

    pub async fn start_turn(&self, params: Value) -> Result<()> {
        self.call(|response| DriverCommand::StartTurn { params, response })
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
    let (turns, turn_receiver) = mpsc::channel(TURN_QUEUE_CAPACITY);
    let turn_worker = tokio::task::spawn_local(drive_turns(
        provider,
        Rc::clone(&connection),
        model.to_owned(),
        turn_receiver,
        Arc::clone(events),
    ));
    while let Some(command) = commands.recv().await {
        match command {
            DriverCommand::CreateSession { params, response } => {
                let result =
                    create_session(provider, &connection, model, cwd, params, &instructions).await;
                let _ = response.send(result);
            }
            DriverCommand::StartTurn { params, response } => {
                let result = match prepare_turn(provider, params, &instructions) {
                    Ok(turn) => turns
                        .send(turn)
                        .await
                        .map_err(|_| anyhow!("ACP turn worker is unavailable")),
                    Err(error) => Err(error),
                };
                let _ = response.send(result);
            }
        }
    }
    turn_worker.abort();
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

struct PreparedTurn {
    session_id: String,
    prompt: String,
    effort: Option<String>,
}

fn prepare_turn(
    provider: AcpProvider,
    params: Value,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<PreparedTurn> {
    let session_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .with_context(|| format!("{} ACP turn is missing threadId", provider.label()))?
        .to_owned();
    let prompt = prompt::input_text(params.get("input").unwrap_or(&Value::Null));
    let prefix = instructions.borrow_mut().remove(&session_id);
    let prompt = match prefix {
        Some(prefix) => format!("{prefix}\n\n{prompt}"),
        None => prompt,
    };
    let effort = params
        .get("effort")
        .and_then(Value::as_str)
        .and_then(|effort| match provider {
            AcpProvider::Grok => prompt::grok_effort(effort),
            AcpProvider::Copilot => prompt::copilot_effort(effort),
        })
        .map(str::to_owned);
    Ok(PreparedTurn {
        session_id,
        prompt,
        effort,
    })
}

async fn drive_turns(
    provider: AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    model: String,
    mut turns: mpsc::Receiver<PreparedTurn>,
    events: Arc<ThreadEventDispatcher>,
) {
    while let Some(turn) = turns.recv().await {
        let id = acp::SessionId::new(turn.session_id.clone());
        if let Some(effort) = turn.effort {
            let mut meta = Map::new();
            meta.insert("reasoningEffort".to_owned(), Value::String(effort));
            let request =
                acp::SetSessionModelRequest::new(id.clone(), model.clone()).meta(Some(meta));
            if let Err(error) = connection.set_session_model(request).await {
                updates::dispatch_error(
                    &events,
                    &turn.session_id,
                    format!("{} ACP set effort failed: {error:?}", provider.label()),
                );
                continue;
            }
        }
        let request = acp::PromptRequest::new(
            id,
            vec![acp::ContentBlock::Text(acp::TextContent::new(turn.prompt))],
        );
        match connection.prompt(request).await {
            Ok(_) => {
                // ACP handlers are local tasks. Yield so notifications parsed before the
                // prompt response are dispatched before the terminal event.
                tokio::task::yield_now().await;
                events.dispatch(json!({
                    "method":"turn/completed",
                    "params":{"threadId":turn.session_id,"turn":{"status":"completed"}}
                }));
            }
            Err(error) => {
                updates::dispatch_error(&events, &turn.session_id, format!("{error:?}"));
            }
        }
    }
}

#[cfg(test)]
mod tests;
