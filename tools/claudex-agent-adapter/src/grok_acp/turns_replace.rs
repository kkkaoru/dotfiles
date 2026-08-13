use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::sync::oneshot;
use tokio::time::{Instant, sleep, timeout_at};

use super::{AcpProvider, ActiveTurns, InvalidatedSessions, REPLACE_SETTLE_TIMEOUT, cancel_turn};

pub(super) async fn replace_active_turn(
    provider: AcpProvider,
    active_turns: &ActiveTurns,
    invalidated_sessions: &InvalidatedSessions,
    session_id: &str,
) -> Result<()> {
    tracing::info!(
        session_id,
        provider = provider.label(),
        "replacing in-flight ACP turn for a newer request on the same session"
    );
    // One shared budget for cancel ack + active_turns clear. Stacking two
    // REPLACE_SETTLE_TIMEOUT waits doubled mid-turn steering latency.
    let deadline = Instant::now() + REPLACE_SETTLE_TIMEOUT;
    let (response_tx, response_rx) = oneshot::channel();
    cancel_turn(active_turns, session_id, response_tx);
    match timeout_at(deadline, response_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                %error,
                session_id,
                "cancel during same-session replace returned an error; waiting for worker exit"
            );
        }
        Ok(Err(_)) => {
            tracing::warn!(
                session_id,
                "cancel response dropped during same-session replace"
            );
        }
        Err(_) => {
            tracing::warn!(
                session_id,
                "cancel did not settle within {:?}; waiting for active_turns clear",
                REPLACE_SETTLE_TIMEOUT
            );
        }
    }
    wait_for_replace_clear(
        provider,
        active_turns,
        invalidated_sessions,
        session_id,
        deadline,
    )
    .await
}

async fn wait_for_replace_clear(
    provider: AcpProvider,
    active_turns: &ActiveTurns,
    invalidated_sessions: &InvalidatedSessions,
    session_id: &str,
    deadline: Instant,
) -> Result<()> {
    while active_turns.borrow().contains_key(session_id) {
        if Instant::now() >= deadline {
            return invalidate_unsettled_replace(
                provider,
                active_turns,
                invalidated_sessions,
                session_id,
            );
        }
        // Yield so the turn worker on this LocalSet can finish execute_turn.
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(1)).await;
    }
    Ok(())
}

fn invalidate_unsettled_replace(
    provider: AcpProvider,
    active_turns: &ActiveTurns,
    invalidated_sessions: &InvalidatedSessions,
    session_id: &str,
) -> Result<()> {
    invalidated_sessions
        .borrow_mut()
        .insert(session_id.to_owned());
    active_turns.borrow_mut().remove(session_id);
    Err(anyhow!(
        "{} ACP session `{}` was invalidated after replace cancel did not settle",
        provider.label(),
        session_id
    ))
}
