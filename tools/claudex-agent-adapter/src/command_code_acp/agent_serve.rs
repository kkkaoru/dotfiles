use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use agent_client_protocol as acp;
use anyhow::Result;
use tokio::sync::{Mutex, mpsc};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use super::{
    HeadlessAgent, Options,
    progress::relay_client_operations,
};

pub async fn serve(options: Options) -> Result<()> {
    serve_io(options, tokio::io::stdin(), tokio::io::stdout()).await
}

pub(in crate::command_code_acp) async fn serve_io<R, W>(options: Options, stdin: R, stdout: W) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin + 'static,
    W: tokio::io::AsyncWrite + Unpin + 'static,
{
    let (operations, requests) = mpsc::unbounded_channel();
    let agent = HeadlessAgent {
        options,
        operations,
        next_session: Cell::new(0),
        session_cwds: RefCell::new(HashMap::new()),
        cancelled: RefCell::new(HashMap::new()),
        running: RefCell::new(HashMap::new()),
        prompt_lock: Mutex::new(()),
    };
    let (connection, io) =
        acp::AgentSideConnection::new(agent, stdout.compat_write(), stdin.compat(), spawn_local);
    tokio::task::spawn_local(relay_client_operations(connection, requests));
    io.await.map_err(|error| anyhow::anyhow!("{error}"))
}

pub(super) fn spawn_local(future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>) {
    tokio::task::spawn_local(future);
}
