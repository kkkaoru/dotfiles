use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::app_server::{ThreadEventDispatcher, ThreadEvents};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixStream, unix::OwnedWriteHalf},
    process::Child,
    sync::mpsc,
};
use uuid::Uuid;
mod events;
mod process;
mod protocol;
use process::GatewayProcess;
struct ActiveTurn {
    request_id: String,
    cancel: mpsc::UnboundedSender<()>,
}
pub struct PiGateway {
    provider: String,
    model_id: String,
    process: tokio::sync::Mutex<Option<Child>>,
    directory: std::path::PathBuf,
    socket: std::path::PathBuf,
    token: String,
    events: Arc<ThreadEventDispatcher>,
    active: Arc<Mutex<HashMap<String, ActiveTurn>>>,
    pending_request_ids: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
    alive: AtomicBool,
}

impl PiGateway {
    pub(crate) async fn spawn(
        provider: &str,
        model_id: &str,
        extensions: &[String],
    ) -> Result<Arc<Self>> {
        if provider.is_empty() || model_id.is_empty() {
            bail!("Pi gateway provider and modelId must not be empty");
        }
        if provider == "claudex" {
            bail!("Pi gateway recursion rejected provider `claudex`");
        }
        let process = GatewayProcess::spawn(extensions).await?;
        Ok(Arc::new(Self {
            provider: provider.to_owned(),
            model_id: model_id.to_owned(),
            process: tokio::sync::Mutex::new(Some(process.child)),
            directory: process.directory,
            socket: process.socket,
            token: process.token,
            events: Arc::new(ThreadEventDispatcher::default()),
            active: Arc::new(Mutex::new(HashMap::new())),
            pending_request_ids: Arc::new(Mutex::new(HashMap::new())),
            alive: AtomicBool::new(true),
        }))
    }

    pub(crate) fn create_thread(&self) -> Value {
        json!({"thread":{"id":Uuid::new_v4().to_string()}})
    }

    pub(crate) fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        let request_id = Uuid::new_v4().to_string();
        self.pending_request_ids
            .lock()
            .expect("Pi pending request registry poisoned")
            .entry(thread_id.to_owned())
            .or_default()
            .push_back(request_id.clone());
        let pending = Arc::clone(&self.pending_request_ids);
        let reserved_thread = thread_id.to_owned();
        let reserved_request = request_id.clone();
        let release = Box::new(move || {
            release_reserved_request(&pending, &reserved_thread, &reserved_request);
        });
        self.events.subscribe_with_drop(&request_id, Some(release))
    }

    fn take_reserved_request_id(&self, thread_id: &str) -> Result<String> {
        let mut pending = self
            .pending_request_ids
            .lock()
            .map_err(|_| anyhow!("Pi pending request registry poisoned"))?;
        let queue = pending.get_mut(thread_id).with_context(|| {
            format!("Pi turn has no reserved subscriber for thread {thread_id}")
        })?;
        let request_id = queue.pop_front().with_context(|| {
            format!("Pi turn has no reserved subscriber for thread {thread_id}")
        })?;
        if queue.is_empty() {
            pending.remove(thread_id);
        }
        Ok(request_id)
    }

    pub(crate) async fn start_turn(self: &Arc<Self>, params: Value) -> Result<()> {
        let thread_id = required_string(&params, "threadId")?.to_owned();
        let raw_request = params
            .get("claudexRequest")
            .context("Pi turn omitted claudexRequest")?;
        let effort = params.get("effort").and_then(Value::as_str);
        let request_id = self.take_reserved_request_id(&thread_id)?;
        let request = protocol::request(
            &request_id,
            &self.token,
            &self.provider,
            &self.model_id,
            raw_request,
            effort,
        )?;
        tracing::debug!(
            thread_id,
            request_id,
            provider = %self.provider,
            model_id = %self.model_id,
            message_count = request["messages"].as_array().map_or(0, Vec::len),
            message_roles = ?request["messages"].as_array().map(|messages| messages.iter().map(|message| message["role"].as_str().unwrap_or("<missing>")).collect::<Vec<_>>()),
            tool_count = request["tools"].as_array().map_or(0, Vec::len),
            "starting Pi gateway turn"
        );
        let (cancel, cancel_rx) = mpsc::unbounded_channel();
        self.active
            .lock()
            .map_err(|_| anyhow!("Pi active-turn registry poisoned"))?
            .insert(
                thread_id.clone(),
                ActiveTurn {
                    request_id: request_id.clone(),
                    cancel,
                },
            );
        let gateway = Arc::clone(self);
        tokio::spawn(async move {
            gateway
                .run_spawned_turn(thread_id, request_id, request, cancel_rx)
                .await;
        });
        Ok(())
    }

    async fn run_spawned_turn(
        &self,
        thread_id: String,
        request_id: String,
        request: Value,
        cancel_rx: mpsc::UnboundedReceiver<()>,
    ) {
        let result = self
            .drive_turn(&thread_id, &request_id, request, cancel_rx)
            .await;
        self.remove_active(&thread_id, &request_id);
        if let Err(error) = result {
            self.alive.store(false, Ordering::Release);
            self.dispatch_error_to(&request_id, &thread_id, &format!("{error:#}"));
        }
    }

    async fn drive_turn(
        &self,
        thread_id: &str,
        request_id: &str,
        request: Value,
        mut cancel_rx: mpsc::UnboundedReceiver<()>,
    ) -> Result<()> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .with_context(|| format!("connect Pi gateway {}", self.socket.display()))?;
        let (reader, mut writer) = stream.into_split();
        write_line(&mut writer, &protocol::hello(&self.token)).await?;
        let mut lines = BufReader::new(reader).lines();
        let ready = read_json_line(&mut lines).await?;
        protocol::validate_ready(&ready)?;
        write_line(&mut writer, &request).await?;
        let mut tools = HashMap::new();
        loop {
            tokio::select! {
                cancel = cancel_rx.recv() => {
                    if cancel.is_some() {
                        write_line(&mut writer, &protocol::cancel(request_id, &self.token)).await?;
                    }
                }
                line = lines.next_line() => {
                    let line = line.context("read Pi gateway event")?
                        .context("Pi gateway closed before a terminal event")?;
                    let event: Value = serde_json::from_str(&line)
                        .context("decode Pi gateway event JSON")?;
                    if self.handle_event(thread_id, request_id, &event, &mut tools)? {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn remove_active(&self, thread_id: &str, request_id: &str) {
        if let Ok(mut active) = self.active.lock()
            && active
                .get(thread_id)
                .is_some_and(|turn| turn.request_id == request_id)
        {
            active.remove(thread_id);
        }
    }

    pub(crate) fn cancel_turn(&self, thread_id: &str) -> Result<()> {
        let active = self
            .active
            .lock()
            .map_err(|_| anyhow!("Pi active-turn registry poisoned"))?;
        if let Some(turn) = active.get(thread_id) {
            let _ = turn.cancel.send(());
        }
        Ok(())
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn cancel_active_turns(&self) {
        let Ok(active) = self.active.lock() else {
            return;
        };
        active.values().for_each(|turn| {
            let _ = turn.cancel.send(());
        });
    }

    pub(crate) async fn shutdown(&self) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        self.cancel_active_turns();
        self.pending_request_ids
            .lock()
            .expect("Pi pending request registry poisoned")
            .clear();
        let child = self.process.lock().await.take();
        if let Some(child) = child {
            GatewayProcess {
                child,
                directory: self.directory.clone(),
                socket: self.socket.clone(),
                token: self.token.clone(),
            }
            .shutdown()
            .await;
        }
        self.events.close();
    }
}

fn release_reserved_request(
    pending: &Mutex<HashMap<String, VecDeque<String>>>,
    thread_id: &str,
    request_id: &str,
) {
    let Ok(mut pending) = pending.lock() else {
        return;
    };
    let remove_thread = pending.get_mut(thread_id).is_some_and(|queue| {
        queue.retain(|reserved| reserved != request_id);
        queue.is_empty()
    });
    if remove_thread {
        pending.remove(thread_id);
    }
}

async fn write_line(writer: &mut OwnedWriteHalf, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).context("encode Pi gateway JSON")?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .context("write Pi gateway JSON line")
}

async fn read_json_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> Result<Value> {
    let line = lines
        .next_line()
        .await
        .context("read Pi gateway handshake")?
        .context("Pi gateway closed during handshake")?;
    serde_json::from_str(&line).context("decode Pi gateway handshake JSON")
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("Pi gateway params omitted {field}"))
}

#[cfg(test)]
#[path = "pi_gateway_tests.rs"]
mod tests;
