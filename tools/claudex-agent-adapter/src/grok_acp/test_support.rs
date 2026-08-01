use std::sync::{Arc, atomic::AtomicBool};

use tokio::sync::mpsc;

use super::{
    DriverThread, GrokAcp, OUTER_TURN_RESERVE, SESSION_QUEUE_CAPACITY, TURN_QUEUE_CAPACITY,
    connection::AcpProvider,
};
use crate::app_server::events::ThreadEventDispatcher;

impl GrokAcp {
    pub(crate) fn stopped_for_test() -> Arc<Self> {
        let (commands, receiver) = mpsc::channel(1);
        drop(receiver);
        Arc::new(Self {
            provider: AcpProvider::Grok,
            commands,
            session_permits: Arc::new(tokio::sync::Semaphore::new(SESSION_QUEUE_CAPACITY)),
            turn_permits: Arc::new(tokio::sync::Semaphore::new(TURN_QUEUE_CAPACITY)),
            outer_permits: Arc::new(tokio::sync::Semaphore::new(OUTER_TURN_RESERVE)),
            turn_capacity: TURN_QUEUE_CAPACITY,
            events: Arc::new(ThreadEventDispatcher::default()),
            alive: Arc::new(AtomicBool::new(false)),
            driver: DriverThread::completed(),
        })
    }
}
