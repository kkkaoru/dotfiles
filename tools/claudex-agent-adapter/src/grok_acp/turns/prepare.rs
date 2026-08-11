use std::{cell::RefCell, collections::HashMap, rc::Rc};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tokio::sync::oneshot;

use super::super::{connection::AcpProvider, prompt};
use super::{ActiveTurns, CancelRequest, PreparedTurn};

pub(super) fn take_cancellation(
    active_turns: &ActiveTurns,
    session_id: &str,
) -> Result<Option<oneshot::Sender<CancelRequest>>> {
    let mut active_turns = active_turns.borrow_mut();
    let Some(cancellation) = active_turns.get_mut(session_id) else {
        return Ok(None);
    };
    cancellation
        .take()
        .map(Some)
        .ok_or_else(|| anyhow!("ACP session `{session_id}` cancellation is already in progress"))
}

pub(super) fn prepare_turn(
    provider: AcpProvider,
    params: Value,
    permit: tokio::sync::OwnedSemaphorePermit,
    cancellation: oneshot::Receiver<CancelRequest>,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<PreparedTurn> {
    let session_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .with_context(|| format!("{} ACP turn is missing threadId", provider.label()))?
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
        .and_then(|effort| match provider {
            AcpProvider::Grok => None,
            AcpProvider::Configured
            | AcpProvider::ConfiguredLaunchScoped
            | AcpProvider::Copilot => prompt::copilot_effort(effort),
        })
        .map(str::to_owned);
    Ok(PreparedTurn {
        session_id,
        prompt,
        effort,
        cancellation,
        _permit: permit,
    })
}

