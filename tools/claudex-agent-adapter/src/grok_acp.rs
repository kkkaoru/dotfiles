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
use tokio::{process::Command, sync::oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::app_server::{ThreadEvents, events::ThreadEventDispatcher};

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
    commands: tokio::sync::mpsc::UnboundedSender<DriverCommand>,
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
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
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
            .map_err(|_| anyhow!("Grok ACP driver is unavailable"))?;
        response_rx
            .await
            .context("Grok ACP driver dropped its response")?
    }
}

struct AcpClient {
    events: Arc<ThreadEventDispatcher>,
}

// Rust nightly branch instrumentation currently emits an invalid mapping for
// async-trait's generated client shim. Stable line coverage still measures it.
#[cfg_attr(coverage_nightly, coverage(off))]
#[async_trait::async_trait(?Send)]
impl acp::Client for AcpClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Ok(permission_response(&request))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        dispatch_notification(&self.events, notification);
        Ok(())
    }
}

fn permission_response(request: &acp::RequestPermissionRequest) -> acp::RequestPermissionResponse {
    let outcome = request
        .options
        .iter()
        .find(|option| option.kind == acp::PermissionOptionKind::AllowOnce)
        .or_else(|| request.options.first())
        .map_or(acp::RequestPermissionOutcome::Cancelled, |option| {
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        });
    acp::RequestPermissionResponse::new(outcome)
}

fn dispatch_notification(events: &ThreadEventDispatcher, notification: acp::SessionNotification) {
    if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update
        && let acp::ContentBlock::Text(text) = chunk.content
        && !text.text.is_empty()
    {
        events.dispatch(json!({
            "method":"item/agentMessage/delta",
            "params":{"threadId":notification.session_id.0,"delta":text.text}
        }));
    }
}

async fn run_driver(
    program: OsString,
    model: String,
    cwd: PathBuf,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<DriverCommand>,
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
    let mut child = Command::new(program)
        .args(["--model", model, "agent", "stdio"])
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
    let client = AcpClient { events };
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
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<DriverCommand>,
    events: &Arc<ThreadEventDispatcher>,
) {
    let instructions = Rc::new(RefCell::new(HashMap::<String, String>::new()));
    while let Some(command) = commands.recv().await {
        match command {
            DriverCommand::CreateSession { params, response } => {
                let result = create_session(&connection, model, cwd, params, &instructions).await;
                let _ = response.send(result);
            }
            DriverCommand::StartTurn { params, response } => {
                let result = start_turn(
                    Rc::clone(&connection),
                    model.to_owned(),
                    params,
                    Rc::clone(&instructions),
                    Arc::clone(events),
                );
                let _ = response.send(result);
            }
        }
    }
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
                .meta(json!({"modelId":model}).as_object().cloned()),
        )
        .await
        .map_err(|error| anyhow!("Grok ACP session/new failed: {error:?}"))?;
    let session_id = response.session_id.0.to_string();
    let base = provider_instructions(&params);
    if !base.is_empty() {
        instructions.borrow_mut().insert(session_id.clone(), base);
    }
    Ok(json!({"thread":{"id":session_id}}))
}

fn start_turn(
    connection: Rc<acp::ClientSideConnection>,
    model: String,
    params: Value,
    instructions: Rc<RefCell<HashMap<String, String>>>,
    events: Arc<ThreadEventDispatcher>,
) -> Result<()> {
    let session_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .context("Grok ACP turn is missing threadId")?
        .to_owned();
    let prompt = input_text(params.get("input").unwrap_or(&Value::Null));
    let prefix = instructions.borrow_mut().remove(&session_id);
    let prompt = prefix.map_or(prompt.clone(), |prefix| format!("{prefix}\n\n{prompt}"));
    let effort = params
        .get("effort")
        .and_then(Value::as_str)
        .and_then(grok_effort)
        .map(str::to_owned);
    tokio::task::spawn_local(async move {
        let id = acp::SessionId::new(session_id.clone());
        if let Some(effort) = effort {
            let mut meta = Map::new();
            meta.insert("reasoningEffort".to_owned(), Value::String(effort));
            let request = acp::SetSessionModelRequest::new(id.clone(), model).meta(Some(meta));
            if let Err(error) = connection.set_session_model(request).await {
                dispatch_error(
                    &events,
                    &session_id,
                    format!("set effort failed: {error:?}"),
                );
                return;
            }
        }
        let request = acp::PromptRequest::new(
            id,
            vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
        );
        match connection.prompt(request).await {
            Ok(_) => {
                // ACP handlers are local tasks. Yield so notifications parsed before the
                // prompt response are dispatched before the terminal event.
                tokio::task::yield_now().await;
                events.dispatch(json!({
                    "method":"turn/completed",
                    "params":{"threadId":session_id,"turn":{"status":"completed"}}
                }));
            }
            Err(error) => dispatch_error(&events, &session_id, format!("{error:?}")),
        }
    });
    Ok(())
}

fn dispatch_error(events: &ThreadEventDispatcher, session_id: &str, message: String) {
    events.dispatch(json!({
        "method":"error",
        "params":{
            "threadId":session_id,
            "willRetry":false,
            "error":{"message":message}
        }
    }));
}

fn provider_instructions(params: &Value) -> String {
    let base = params
        .get("baseInstructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let adapter = params
        .get("developerInstructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    base.strip_suffix(adapter)
        .unwrap_or(base)
        .trim_end_matches(['\n', ' '])
        .to_owned()
}

fn input_text(input: &Value) -> String {
    match input {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn grok_effort(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "mid" | "medium" => Some("medium"),
        "high" | "xhigh" | "max" => Some("high"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
