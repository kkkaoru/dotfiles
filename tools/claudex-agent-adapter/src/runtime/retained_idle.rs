use std::{net::SocketAddr, sync::Arc, time::Duration};

use crate::{anthropic::Bridge, listen_handover::ListenHandover, sticky_grace::STICKY_IDLE_GRACE};

const POLL: Duration = Duration::from_secs(30);

pub(super) fn rebound_generation_should_exit(
    advertised: SocketAddr,
    service: SocketAddr,
    rebound_for: Duration,
    busy_sessions: usize,
) -> bool {
    advertised != service && busy_sessions == 0 && rebound_for >= STICKY_IDLE_GRACE
}

pub(super) async fn rebound_idle_exit(handover: ListenHandover, bridge: Arc<Bridge>) {
    let service = handover.service_addr();
    let mut rebound_since = None::<tokio::time::Instant>;
    loop {
        tokio::time::sleep(POLL).await;
        let advertised = handover.advertised_addr();
        if advertised == service {
            rebound_since = None;
            continue;
        }
        let since = *rebound_since.get_or_insert_with(tokio::time::Instant::now);
        let busy = bridge.busy_claude_session_ids().await.len();
        if !rebound_generation_should_exit(advertised, service, since.elapsed(), busy) {
            continue;
        }
        tracing::info!(
            advertised = %advertised,
            "retained generation idle after rebound; shutting down"
        );
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_listener_does_not_self_exit() {
        let listen = "127.0.0.1:8318".parse().unwrap();
        assert!(!rebound_generation_should_exit(
            listen,
            listen,
            STICKY_IDLE_GRACE,
            0
        ));
    }

    #[test]
    fn rebound_stays_until_idle_grace() {
        let advertised = "127.0.0.1:61915".parse().unwrap();
        let service = "127.0.0.1:8318".parse().unwrap();
        assert!(!rebound_generation_should_exit(
            advertised,
            service,
            STICKY_IDLE_GRACE - Duration::from_secs(1),
            0
        ));
        assert!(!rebound_generation_should_exit(
            advertised,
            service,
            STICKY_IDLE_GRACE,
            1
        ));
        assert!(rebound_generation_should_exit(
            advertised,
            service,
            STICKY_IDLE_GRACE,
            0
        ));
    }
}
