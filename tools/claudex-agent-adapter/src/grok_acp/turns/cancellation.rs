use std::{error::Error, fmt, future::Future, time::Duration};

#[cfg(test)]
use agent_client_protocol as acp;

use super::{ActiveTurns, CancelRequest, InvalidatedSessions, dispatch_turn_terminal};
use crate::{app_server::events::ThreadEventDispatcher, grok_acp::connection::AcpProvider};

const CANCELLATION_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct CancelCtx<'a> {
    pub(super) provider: AcpProvider,
    pub(super) session_id: &'a str,
    pub(super) permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) cancellation: CancelRequest,
    pub(super) events: &'a ThreadEventDispatcher,
    pub(super) invalidated_sessions: &'a InvalidatedSessions,
}

#[derive(Clone, Copy)]
pub(super) struct SettlementPolicy {
    timeout: Duration,
}

impl Default for SettlementPolicy {
    fn default() -> Self {
        Self {
            timeout: CANCELLATION_SETTLEMENT_TIMEOUT,
        }
    }
}

pub(super) enum Settlement<T> {
    Settled(T),
    TimedOut,
}

impl SettlementPolicy {
    async fn settle<F, T>(self, future: F) -> Settlement<T>
    where
        F: Future<Output = T>,
    {
        match tokio::time::timeout(self.timeout, future).await {
            Ok(value) => Settlement::Settled(value),
            Err(_) => Settlement::TimedOut,
        }
    }
}

#[derive(Debug)]
struct CancellationSettlementTimeout {
    provider: AcpProvider,
    session_id: String,
    timeout: Duration,
}

impl fmt::Display for CancellationSettlementTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ACP session `{}` cancellation did not settle within {:?}",
            self.provider.label(),
            self.session_id,
            self.timeout
        )
    }
}

impl Error for CancellationSettlementTimeout {}

#[derive(Debug)]
struct SetupCancellationSettlementTimeout {
    provider: AcpProvider,
    session_id: String,
    timeout: Duration,
}

impl fmt::Display for SetupCancellationSettlementTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ACP session `{}` setup cancellation did not settle within {:?}",
            self.provider.label(),
            self.session_id,
            self.timeout
        )
    }
}

impl Error for SetupCancellationSettlementTimeout {}

#[path = "cancellation_settle.rs"]
mod settle;
use settle::{continue_after_cancel_request, settle_cancelled_prompt};

#[path = "cancellation_run.rs"]
mod run;
pub(super) use run::{cancel_prompt, cancel_setup, finish_setup_cancellation};

#[cfg(test)]
include!("cancellation_tests.rs");
