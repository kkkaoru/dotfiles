use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::{
    agent_backend::AcpLaunch,
    app_server::{ThreadEvents, events::ThreadEventDispatcher},
};

mod cancel_settle;
mod client;
mod configured;
mod connection;
mod driver;
mod plugin;
mod prompt;
mod queue;
mod session;
#[cfg(test)]
mod test_support;
mod turns;
mod updates;

const COMMAND_QUEUE_CAPACITY: usize = 32;
const SESSION_QUEUE_CAPACITY: usize = 1;
const TURN_QUEUE_CAPACITY: usize = 8;
const CONFIGURED_TURN_QUEUE_CAPACITY: usize = 2;
/// Reserved so SubAgent bursts cannot starve interactive user turns.
const OUTER_TURN_RESERVE: usize = 1;
pub(crate) const MAX_MODEL_CONCURRENCY: usize =
    tokio::sync::Semaphore::MAX_PERMITS - OUTER_TURN_RESERVE;
pub(crate) const DEFAULT_REASONING_EFFORT: &str = "high";

use connection::AcpProvider;
use driver::run_driver;
use turns::acquire_turn_permit;

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
    Shutdown {
        response: oneshot::Sender<()>,
    },
}

struct DriverSetup {
    provider: AcpProvider,
    program: OsString,
    arguments: Option<Vec<String>>,
    model: String,
    effort: Option<String>,
    cwd: PathBuf,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
    ready: oneshot::Sender<Result<()>>,
}

struct DriverThread {
    handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    joined: tokio::sync::watch::Sender<bool>,
}

fn finish_driver_thread(
    handle: std::thread::JoinHandle<()>,
    joined: tokio::sync::watch::Sender<bool>,
) -> std::thread::Result<()> {
    let result = handle.join();
    joined.send_replace(true);
    result
}

async fn join_driver_thread(
    handle: std::thread::JoinHandle<()>,
    joined: tokio::sync::watch::Sender<bool>,
) {
    let join = tokio::task::spawn_blocking(move || finish_driver_thread(handle, joined));
    if let Err(error) = join.await {
        tracing::error!(?error, "failed to join ACP driver thread");
    }
}

impl DriverThread {
    fn new(handle: std::thread::JoinHandle<()>) -> Self {
        let (joined, _) = tokio::sync::watch::channel(false);
        Self {
            handle: std::sync::Mutex::new(Some(handle)),
            joined,
        }
    }

    async fn join(&self) {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            join_driver_thread(handle, self.joined.clone()).await;
            return;
        }

        if *self.joined.borrow() {
            return;
        }
        let mut joined = self.joined.subscribe();
        if !*joined.borrow_and_update() {
            let _ = joined.changed().await;
        }
    }

    #[cfg(test)]
    fn completed() -> Self {
        let (joined, _) = tokio::sync::watch::channel(true);
        Self {
            handle: std::sync::Mutex::new(None),
            joined,
        }
    }

    #[cfg(test)]
    fn is_joined(&self) -> bool {
        *self.joined.borrow()
    }
}

pub struct GrokAcp {
    provider: AcpProvider,
    commands: mpsc::Sender<DriverCommand>,
    session_permits: Arc<tokio::sync::Semaphore>,
    turn_permits: Arc<tokio::sync::Semaphore>,
    outer_permits: Arc<tokio::sync::Semaphore>,
    turn_capacity: usize,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
    driver: DriverThread,
}

impl GrokAcp {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        Self::spawn_with_effort(model, DEFAULT_REASONING_EFFORT).await
    }

    pub async fn spawn_with_effort(model: &str, effort: &str) -> Result<Arc<Self>> {
        let program = std::env::var_os("CLAUDEX_GROK_PROGRAM").unwrap_or_else(|| "grok".into());
        let cwd = std::env::current_dir().context("resolve Grok ACP working directory")?;
        Self::spawn_provider(
            AcpProvider::Grok,
            model,
            Some(effort),
            program,
            None,
            cwd,
            None,
        )
        .await
    }

    pub async fn spawn_copilot(model: &str) -> Result<Arc<Self>> {
        let program =
            std::env::var_os("CLAUDEX_COPILOT_PROGRAM").unwrap_or_else(|| "copilot".into());
        let cwd = std::env::current_dir().context("resolve Copilot ACP working directory")?;
        Self::spawn_provider(AcpProvider::Copilot, model, None, program, None, cwd, None).await
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_with_program_and_effort(model, DEFAULT_REASONING_EFFORT, program, cwd).await
    }

    pub async fn spawn_with_program_and_effort(
        model: &str,
        effort: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(
            AcpProvider::Grok,
            model,
            Some(effort),
            program,
            None,
            cwd,
            None,
        )
        .await
    }

    pub async fn spawn_copilot_with_program(
        model: &str,
        program: impl Into<OsString>,
        cwd: PathBuf,
    ) -> Result<Arc<Self>> {
        Self::spawn_provider(AcpProvider::Copilot, model, None, program, None, cwd, None).await
    }

    async fn spawn_provider(
        provider: AcpProvider,
        model: &str,
        effort: Option<&str>,
        program: impl Into<OsString>,
        arguments: Option<Vec<String>>,
        cwd: PathBuf,
        max_concurrency: Option<usize>,
    ) -> Result<Arc<Self>> {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let session_capacity = match provider {
            AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped => {
                max_concurrency.unwrap_or(SESSION_QUEUE_CAPACITY)
            }
            AcpProvider::Grok | AcpProvider::Copilot => SESSION_QUEUE_CAPACITY,
        };
        let session_permits = Arc::new(tokio::sync::Semaphore::new(session_capacity));
        let default_turn_capacity = match provider {
            AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped => {
                CONFIGURED_TURN_QUEUE_CAPACITY
            }
            AcpProvider::Grok | AcpProvider::Copilot => TURN_QUEUE_CAPACITY,
        };
        let turn_capacity =
            max_concurrency.map_or(default_turn_capacity, |limit| limit + OUTER_TURN_RESERVE);
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
        let effort = effort.map(str::to_owned);
        let program = program.into();
        let driver = std::thread::Builder::new()
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
                        effort,
                        cwd,
                        events: driver_events,
                        alive: driver_alive,
                        ready: ready_tx,
                    },
                    command_rx,
                )));
            })
            .with_context(|| format!("start {} ACP driver thread", provider.label()))?;
        let driver = DriverThread::new(driver);
        let ready = ready_rx
            .await
            .with_context(|| format!("{} ACP driver stopped during startup", provider.label()))
            .and_then(|ready| ready);
        if let Err(error) = ready {
            driver.join().await;
            return Err(error);
        }
        Ok(Arc::new(Self {
            provider,
            commands: command_tx,
            session_permits,
            turn_permits,
            outer_permits,
            turn_capacity,
            events,
            alive,
            driver,
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
        let permit = queue::acquire(self.provider, "session/new", async {
            Arc::clone(&self.session_permits)
                .acquire_owned()
                .await
                .map_err(|_| anyhow!("ACP driver is unavailable"))
        })
        .await?;
        self.call(|response| DriverCommand::CreateSession {
            params,
            _permit: permit,
            response,
        })
        .await
    }

    pub async fn start_turn(&self, params: Value) -> Result<()> {
        let is_user = params.get("priority").and_then(Value::as_str) == Some("user");
        let permit = queue::acquire(
            self.provider,
            "turn/start",
            acquire_turn_permit(&self.turn_permits, &self.outer_permits, is_user),
        )
        .await?;
        self.call(|response| DriverCommand::StartTurn {
            params,
            permit,
            response,
        })
        .await
    }

    pub async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        if let Err(error) = self
            .call(|response| DriverCommand::CancelTurn {
                session_id: session_id.to_owned(),
                response,
            })
            .await
        {
            return cancel_settle::settle_cancel_after_driver_loss(session_id, error);
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
        let (response, stopped) = oneshot::channel();
        if self
            .commands
            .send(DriverCommand::Shutdown { response })
            .await
            .is_ok()
        {
            let _ = stopped.await;
        }
        self.driver.join().await;
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

#[cfg(test)]
mod tests;
