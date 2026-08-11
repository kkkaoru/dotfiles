use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
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
mod driver_types;
mod plugin;
mod prompt;
mod queue;
mod session;
mod spawn;
#[cfg(test)]
mod test_support;
mod turns;
mod updates;

const COMMAND_QUEUE_CAPACITY: usize = 32;
const SESSION_QUEUE_CAPACITY: usize = 1;
const TURN_QUEUE_CAPACITY: usize = 8;
/// When providers omit maxConcurrency, configured ACP used to fall back to a
/// single session slot (near-serial). Match the common qwen default of 3 so
/// cursor/deepseek/opencode routes can overlap without per-route config.
pub(crate) const DEFAULT_CONFIGURED_MAX_CONCURRENCY: usize = 3;
/// Reserved so SubAgent bursts cannot starve interactive user turns.
const OUTER_TURN_RESERVE: usize = 1;
pub(crate) const MAX_MODEL_CONCURRENCY: usize =
    tokio::sync::Semaphore::MAX_PERMITS - OUTER_TURN_RESERVE;
pub(crate) const DEFAULT_REASONING_EFFORT: &str = "high";

use connection::AcpProvider;
#[allow(unused_imports)]
use driver_types::{DriverCommand, DriverSetup, DriverThread};
use turns::acquire_turn_permit;

#[cfg(test)]
use turns::{CancelRequest, PreparedTurn};

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
