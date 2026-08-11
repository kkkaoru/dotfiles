use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::{
    HandoverCommand, HandoverListener, ListenHandover,
    support::{accept_or_backoff, ephemeral_bind_addr, write_rebind_state},
};

impl HandoverListener {
    pub(crate) fn new(
        listener: TcpListener,
        handover: &ListenHandover,
        request: watch::Receiver<HandoverCommand>,
    ) -> Self {
        Self {
            listener,
            advertised: Arc::clone(&handover.advertised),
            request,
            cache: handover.cache.clone(),
            canonical: handover.canonical,
        }
    }

    async fn apply(&mut self, command: HandoverCommand) -> Result<()> {
        let target = match command {
            HandoverCommand::None => return Ok(()),
            HandoverCommand::Ephemeral => ephemeral_bind_addr(self.canonical),
            HandoverCommand::Bind(listen) => listen,
        };
        let next = TcpListener::bind(target)
            .await
            .context("bind listener during handover")?;
        let listen = next.local_addr().context("read rebound listener")?;
        write_rebind_state(&self.cache, self.canonical, listen)?;
        self.listener = next;
        *self.advertised.write().expect("listen handover lock") = listen;
        tracing::info!(%listen, "adapter listener rebound");
        Ok(())
    }

    async fn apply_changed_request(
        &mut self,
        changed: Result<(), tokio::sync::watch::error::RecvError>,
    ) {
        if changed.is_err() {
            return;
        }
        let command = self.request.borrow().clone();
        if let Err(error) = self.apply(command).await {
            tracing::error!(%error, "listener handover failed");
        }
    }
}

impl axum::serve::Listener for HandoverListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                biased;
                changed = self.request.changed() => {
                    self.apply_changed_request(changed).await;
                }
                result = self.listener.accept() => {
                    if let Some(accepted) = accept_or_backoff(result).await {
                        return accepted;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(*self.advertised.read().expect("listen handover lock"))
    }
}
