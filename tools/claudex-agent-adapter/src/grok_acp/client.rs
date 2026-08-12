use std::sync::Arc;

use agent_client_protocol::{self as acp};

use super::updates::{self, ThoughtUnits};
use crate::app_server::events::ThreadEventDispatcher;

pub(super) struct AcpClient {
    events: Arc<ThreadEventDispatcher>,
    thoughts: Arc<ThoughtUnits>,
}

impl AcpClient {
    pub(super) fn new(events: Arc<ThreadEventDispatcher>) -> Self {
        Self {
            events,
            thoughts: Arc::new(ThoughtUnits::default()),
        }
    }

    fn permission_response(
        request: &acp::RequestPermissionRequest,
    ) -> acp::RequestPermissionResponse {
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

    fn handle_session_notification(&self, notification: acp::SessionNotification) {
        updates::dispatch_notification(&self.events, &self.thoughts, notification);
    }

    fn handle_extension_notification(&self, notification: acp::ExtNotification) {
        updates::dispatch_extension(&self.events, &self.thoughts, notification);
    }
}

// Rust nightly branch instrumentation currently emits an invalid mapping for
// async-trait's generated client shim. The inherent handlers above remain
// instrumented and are covered by deterministic notification fixtures.
// coverage-exception: async-trait-codegen; symbol=impl acp::Client for AcpClient; evidence=grok_acp::tests::client_inherent_handlers_cover_permissions_and_notifications
#[cfg_attr(coverage_nightly, coverage(off))]
#[async_trait::async_trait(?Send)]
impl acp::Client for AcpClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Ok(Self::permission_response(&request))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        self.handle_session_notification(notification);
        Ok(())
    }

    async fn ext_notification(&self, notification: acp::ExtNotification) -> acp::Result<()> {
        self.handle_extension_notification(notification);
        Ok(())
    }
}
