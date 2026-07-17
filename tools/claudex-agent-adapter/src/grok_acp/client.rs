use std::sync::Arc;

use agent_client_protocol::{self as acp};

use super::updates;
use crate::app_server::events::ThreadEventDispatcher;

pub(super) struct AcpClient {
    events: Arc<ThreadEventDispatcher>,
}

impl AcpClient {
    pub(super) fn new(events: Arc<ThreadEventDispatcher>) -> Self {
        Self { events }
    }
}

// Rust nightly branch instrumentation currently emits an invalid mapping for
// async-trait's generated client shim. Stable line coverage still measures it.
#[cfg_attr(coverage_nightly, coverage(off))]
#[async_trait::async_trait(?Send)]
impl acp::Client for AcpClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Ok(permission_response(&request))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        updates::dispatch_notification(&self.events, notification);
        Ok(())
    }

    async fn ext_notification(&self, notification: acp::ExtNotification) -> acp::Result<()> {
        updates::dispatch_extension(&self.events, notification);
        Ok(())
    }
}

fn permission_response(request: &acp::RequestPermissionRequest) -> acp::RequestPermissionResponse {
    let outcome = request
        .options
        .iter()
        .find(|option| option.kind == acp::PermissionOptionKind::AllowOnce)
        .or_else(|| request.options.first())
        .map_or(acp::RequestPermissionOutcome::Cancelled, |option| {
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        });
    acp::RequestPermissionResponse::new(outcome)
}
