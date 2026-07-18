use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use tokio::{
    process::Command,
    sync::{mpsc, oneshot},
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::app_server::{ThreadEvents, events::ThreadEventDispatcher};

mod client;
mod plugin;
mod prompt;
mod updates;

const COMMAND_QUEUE_CAPACITY: usize = 32;
const TURN_QUEUE_CAPACITY: usize = 8;

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

pub struct GrokAcp {
    commands: mpsc::Sender<DriverCommand>,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
}

impl GrokAcp {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        let program = std::env::var_os("CLAUDEX_GROK_PROGRAM").unwrap_or_else(|| "grok".into());
        let cwd = std::env::current_dir().context("resolve Grok ACP working directory")?;
        Self::spawn_with_program(model, program, cwd).await
    }

    pub async fn spawn_with_program(
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
            .name("claudex-grok-acp".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build Grok ACP runtime");
                let local = tokio::task::LocalSet::new();
                runtime.block_on(local.run_until(run_driver(
                    program,
                    model,
                    cwd,
                    command_rx,
                    driver_events,
                    driver_alive,
                    ready_tx,
                )));
            })
            .context("start Grok ACP driver thread")?;
        ready_rx
            .await
            .context("Grok ACP driver stopped during startup")??;
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
            .map_err(|_| anyhow!("Grok ACP driver is unavailable"))?;
        response_rx
            .await
            .context("Grok ACP driver dropped its response")?
    }
}

async fn run_driver(
    program: OsString,
    model: String,
    cwd: PathBuf,
    mut commands: mpsc::Receiver<DriverCommand>,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
    ready: oneshot::Sender<Result<()>>,
) {
    let started = start_connection(&program, &model, &cwd, Arc::clone(&events)).await;
    let Ok((connection, child)) = started else {
        let _ = ready.send(started.map(|_| ()));
        alive.store(false, Ordering::Relaxed);
        events.close();
        return;
    };
    let _ = ready.send(Ok(()));
    drive_commands(
        Rc::new(connection),
        child,
        &model,
        &cwd,
        &mut commands,
        &events,
    )
    .await;
    alive.store(false, Ordering::Relaxed);
    events.close();
}

async fn start_connection(
    program: &OsString,
    model: &str,
    cwd: &Path,
    events: Arc<ThreadEventDispatcher>,
) -> Result<(acp::ClientSideConnection, tokio::process::Child)> {
    let plugin_dir = plugin::prepare(program)?;
    let mut command = Command::new(program);
    command.args(["--model", model, "agent"]);
    if let Some(path) = plugin_dir {
        command.arg("--plugin-dir").arg(path);
    }
    let mut child = command
        .arg("stdio")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("start `grok agent stdio`")?;
    let outgoing = child
        .stdin
        .take()
        .context("Grok ACP stdin is unavailable")?
        .compat_write();
    let incoming = child
        .stdout
        .take()
        .context("Grok ACP stdout is unavailable")?
        .compat();
    let client = client::AcpClient::new(events);
    let (connection, handle_io) =
        acp::ClientSideConnection::new(client, outgoing, incoming, |future| {
            tokio::task::spawn_local(future);
        });
    tokio::task::spawn_local(async move {
        if let Err(error) = handle_io.await {
            tracing::error!(?error, "Grok ACP I/O stopped");
        }
    });
    initialize(&connection).await?;
    Ok((connection, child))
}

async fn initialize(connection: &acp::ClientSideConnection) -> Result<()> {
    let response = connection
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                .client_info(acp::Implementation::new(
                    "claudex-agent-adapter",
                    env!("CARGO_PKG_VERSION"),
                ))
                .meta(
                    json!({
                        "startupHints": {
                            "nonInteractive": true,
                            "skipGitStatus": true,
                            "skipProjectLayout": true
                        },
                        "clientType":"claudex-agent-adapter"
                    })
                    .as_object()
                    .cloned(),
                ),
        )
        .await
        .map_err(|error| anyhow!("Grok ACP initialize failed: {error:?}"))?;
    if response.protocol_version != acp::ProtocolVersion::V1 {
        bail!("Grok ACP selected unsupported protocol version")
    }
    let preferred = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("defaultAuthMethodId"))
        .and_then(Value::as_str);
    let method = preferred
        .and_then(|id| {
            response
                .auth_methods
                .iter()
                .find(|method| method.id().0.as_ref() == id)
        })
        .or_else(|| response.auth_methods.first());
    if let Some(method) = method {
        connection
            .authenticate(
                acp::AuthenticateRequest::new(method.id().clone())
                    .meta(json!({"headless":true}).as_object().cloned()),
            )
            .await
            .map_err(|error| anyhow!("Grok ACP authentication failed: {error:?}"))?;
    }
    Ok(())
}

async fn drive_commands(
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
        Rc::clone(&connection),
        model.to_owned(),
        turn_receiver,
        Arc::clone(events),
    ));
    while let Some(command) = commands.recv().await {
        match command {
            DriverCommand::CreateSession { params, response } => {
                let result = create_session(&connection, model, cwd, params, &instructions).await;
                let _ = response.send(result);
            }
            DriverCommand::StartTurn { params, response } => {
                let result = match prepare_turn(params, &instructions) {
                    Ok(turn) => turns
                        .send(turn)
                        .await
                        .map_err(|_| anyhow!("Grok ACP turn worker is unavailable")),
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
        .map_err(|error| anyhow!("Grok ACP session/new failed: {error:?}"))?;
    let session_id = response.session_id.0.to_string();
    let base = prompt::provider_instructions(&params);
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
    params: Value,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<PreparedTurn> {
    let session_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .context("Grok ACP turn is missing threadId")?
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
        .and_then(prompt::grok_effort)
        .map(str::to_owned);
    Ok(PreparedTurn {
        session_id,
        prompt,
        effort,
    })
}

async fn drive_turns(
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
                    format!("set effort failed: {error:?}"),
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
