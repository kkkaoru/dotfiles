use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, oneshot};

use crate::app_server::events::ThreadEventDispatcher;

use super::super::connection::AcpProvider;
use super::super::driver::run_driver;
use super::super::driver_types::{DriverSetup, DriverThread};
use super::super::{
    COMMAND_QUEUE_CAPACITY, DEFAULT_CONFIGURED_MAX_CONCURRENCY, GrokAcp, OUTER_TURN_RESERVE,
    SESSION_QUEUE_CAPACITY, TURN_QUEUE_CAPACITY,
};

/// How many concurrent `session/new` calls this ACP provider permits.
pub(in crate::grok_acp) fn session_create_capacity(
    provider: AcpProvider,
    max_concurrency: Option<usize>,
) -> usize {
    match provider {
        AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped => {
            max_concurrency.unwrap_or(DEFAULT_CONFIGURED_MAX_CONCURRENCY)
        }
        // Honor route maxConcurrency for session/new too. Leaving this at 1
        // while turn permits scale serialized parallel SubAgent create_session.
        AcpProvider::Grok | AcpProvider::Copilot => {
            max_concurrency.unwrap_or(SESSION_QUEUE_CAPACITY).max(1)
        }
    }
}

impl GrokAcp {
    pub(in crate::grok_acp) async fn spawn_provider(
        provider: AcpProvider,
        model: &str,
        effort: Option<&str>,
        program: impl Into<OsString>,
        arguments: Option<Vec<String>>,
        cwd: PathBuf,
        max_concurrency: Option<usize>,
    ) -> Result<Arc<Self>> {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let session_capacity = session_create_capacity(provider, max_concurrency);
        let session_permits = Arc::new(tokio::sync::Semaphore::new(session_capacity));
        let turn_capacity = match provider {
            AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped => {
                max_concurrency.unwrap_or(DEFAULT_CONFIGURED_MAX_CONCURRENCY) + OUTER_TURN_RESERVE
            }
            AcpProvider::Grok | AcpProvider::Copilot => {
                max_concurrency.map_or(TURN_QUEUE_CAPACITY, |limit| limit + OUTER_TURN_RESERVE)
            }
        };
        let turn_permits = Arc::new(tokio::sync::Semaphore::new(
            turn_capacity.saturating_sub(OUTER_TURN_RESERVE).max(1),
        ));
        let outer_permits = Arc::new(tokio::sync::Semaphore::new(OUTER_TURN_RESERVE));
        let events = Arc::new(ThreadEventDispatcher::default());
        let alive = Arc::new(AtomicBool::new(true));
        let cooldown = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = oneshot::channel();
        let driver_events = Arc::clone(&events);
        let driver_alive = Arc::clone(&alive);
        let driver_cooldown = Arc::clone(&cooldown);
        let model = model.to_owned();
        let driver_model = model.clone();
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
                        model: driver_model,
                        effort,
                        cwd,
                        events: driver_events,
                        alive: driver_alive,
                        cooldown: driver_cooldown,
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
            model,
            commands: command_tx,
            session_permits,
            turn_permits,
            outer_permits,
            turn_capacity,
            events,
            alive,
            cooldown,
            driver,
        }))
    }
}
