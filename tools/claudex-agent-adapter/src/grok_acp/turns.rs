use std::future::Future;

use tokio::sync::mpsc;

use super::PreparedTurn;

pub(super) async fn drive_turn_tasks<F, Fut>(mut turns: mpsc::Receiver<PreparedTurn>, mut start: F)
where
    F: FnMut(PreparedTurn) -> Fut,
    Fut: Future<Output = ()> + 'static,
{
    let mut active = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            turn = turns.recv() => match turn {
                Some(turn) => {
                    active.spawn_local(start(turn));
                }
                None => break,
            },
            completed = active.join_next(), if !active.is_empty() => {
                if let Some(Err(error)) = completed {
                    tracing::error!(?error, "ACP turn task stopped unexpectedly");
                }
            }
        }
    }
    active.abort_all();
    while active.join_next().await.is_some() {}
}
