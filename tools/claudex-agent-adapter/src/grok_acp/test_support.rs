use std::sync::{Arc, atomic::AtomicBool};

use tokio::sync::mpsc;

use super::{
    DriverCommand, DriverThread, GrokAcp, OUTER_TURN_RESERVE, SESSION_QUEUE_CAPACITY,
    TURN_QUEUE_CAPACITY, connection::AcpProvider,
};
use crate::app_server::events::ThreadEventDispatcher;

// This module exists only to construct deterministic unit-test providers; it
// is not a production execution path and must not dilute production coverage.
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
impl GrokAcp {
    pub(crate) fn stopped_for_test() -> Arc<Self> {
        Self::for_test(false)
    }

    pub(crate) fn alive_for_test() -> Arc<Self> {
        Self::for_test(true)
    }

    async fn settled_for_test(provider: AcpProvider) -> Arc<Self> {
        let (commands, mut receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    DriverCommand::CancelTurn { response, .. } => {
                        let _ = response.send(Ok(()));
                    }
                    DriverCommand::Shutdown { response } => {
                        let _ = response.send(());
                        break;
                    }
                    DriverCommand::CreateSession { response, .. } => {
                        let _ = response.send(Err(anyhow::anyhow!("test session unavailable")));
                    }
                    DriverCommand::StartTurn { response, .. } => {
                        let _ = response.send(Err(anyhow::anyhow!("test turn unavailable")));
                    }
                }
            }
        });
        Self::for_test_with_commands(provider, commands, true)
    }

    pub(crate) async fn settled_copilot_for_test() -> Arc<Self> {
        Self::settled_for_test(AcpProvider::Copilot).await
    }

    fn for_test(alive: bool) -> Arc<Self> {
        let (commands, receiver) = mpsc::channel(1);
        drop(receiver);
        Self::for_test_with_commands(AcpProvider::Grok, commands, alive)
    }

    fn for_test_with_commands(
        provider: AcpProvider,
        commands: mpsc::Sender<DriverCommand>,
        alive: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            provider,
            commands,
            session_permits: Arc::new(tokio::sync::Semaphore::new(SESSION_QUEUE_CAPACITY)),
            turn_permits: Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY)),
            outer_permits: Arc::new(tokio::sync::Semaphore::new(OUTER_TURN_RESERVE)),
            turn_capacity: TURN_QUEUE_CAPACITY,
            events: Arc::new(ThreadEventDispatcher::default()),
            alive: Arc::new(AtomicBool::new(alive)),
            driver: DriverThread::completed(),
        })
    }
}
