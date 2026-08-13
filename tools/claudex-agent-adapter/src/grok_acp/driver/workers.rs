use std::rc::Rc;

use tokio::sync::{mpsc, oneshot};

use super::super::{
    DriverCommand, session,
    turns::{TurnDriver, cancel_turn, drive_turns},
};
use super::{
    DriverCommandContext, DriverWorkerContext, DriverWorkers, StartTurnRequest, drive_start_turns,
    schedule_start_turn,
};

pub(super) fn spawn_driver_workers(context: DriverWorkerContext) -> DriverWorkers {
    let DriverWorkerContext {
        provider,
        connection,
        model,
        events,
        active_turns,
        invalidated_sessions,
        instructions,
        alive,
        cooldown,
        quota,
    } = context;
    let (turns, turn_receiver) = mpsc::channel(super::super::TURN_QUEUE_CAPACITY);
    let active_turns_for_drive = Rc::clone(&active_turns);
    let invalidated_sessions_for_drive = Rc::clone(&invalidated_sessions);
    let active_turns_for_start = Rc::clone(&active_turns);
    let invalidated_sessions_for_start = Rc::clone(&invalidated_sessions);
    let turn_worker = tokio::task::spawn_local(drive_turns(
        TurnDriver {
            provider,
            connection: Rc::clone(&connection),
            model,
            events,
            active_turns: active_turns_for_drive,
            invalidated_sessions: invalidated_sessions_for_drive,
            alive,
            cooldown,
            quota,
        },
        turn_receiver,
    ));
    let (start_turns, start_turn_receiver) = mpsc::unbounded_channel();
    let start_turn_worker = tokio::task::spawn_local(drive_start_turns(
        provider,
        start_turn_receiver,
        instructions,
        turns.clone(),
        active_turns_for_start,
        invalidated_sessions_for_start,
    ));
    DriverWorkers {
        start_turns,
        turn_worker,
        start_turn_worker,
        turns,
    }
}

pub(super) fn process_driver_command(
    context: DriverCommandContext<'_>,
    command: DriverCommand,
) -> Option<oneshot::Sender<()>> {
    let DriverCommandContext {
        provider,
        connection,
        model,
        cwd,
        instructions,
        workers,
        active_turns,
    } = context;
    match command {
        DriverCommand::CreateSession {
            params,
            _permit: permit,
            response,
        } => {
            session::Task {
                provider,
                connection: Rc::clone(connection),
                model: model.to_owned(),
                cwd: cwd.to_owned(),
                params,
                instructions: Rc::clone(instructions),
                permit,
                response,
            }
            .spawn();
            None
        }
        DriverCommand::StartTurn {
            params,
            permit,
            response,
        } => {
            let request = StartTurnRequest {
                params,
                permit,
                response,
            };
            schedule_start_turn(&workers.start_turns, request);
            None
        }
        DriverCommand::CancelTurn {
            session_id,
            response,
        } => {
            cancel_turn(active_turns, &session_id, response);
            None
        }
        DriverCommand::Shutdown { response } => Some(response),
    }
}

pub(super) async fn stop_driver_workers(workers: DriverWorkers) {
    drop(workers.start_turns);
    workers.start_turn_worker.abort();
    let _ = workers.start_turn_worker.await;
    drop(workers.turns);
    workers.turn_worker.abort();
    let _ = workers.turn_worker.await;
}
