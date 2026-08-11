use agent_client_protocol as acp;

use super::HeadlessAgent;

// Nightly branch instrumentation emits an invalid mapping for async-trait's
// generated Agent shim (same llvm-cov getInstantiationGroups crash as Grok's
// ACP client). Fixture tests still cover the delegated prompt/cancel paths.
#[cfg_attr(coverage_nightly, coverage(off))]
#[async_trait::async_trait(?Send)]
impl acp::Agent for HeadlessAgent {
    async fn initialize(
        &self,
        _request: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1))
    }

    async fn authenticate(
        &self,
        _request: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        Ok(acp::NewSessionResponse::new(self.open_session(request.cwd)))
    }

    async fn prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        self.handle_prompt(request).await
    }

    async fn cancel(&self, request: acp::CancelNotification) -> acp::Result<()> {
        self.handle_cancel(&request.session_id);
        Ok(())
    }

    async fn set_session_model(
        &self,
        _request: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        Ok(acp::SetSessionModelResponse::default())
    }

    async fn set_session_config_option(
        &self,
        _request: acp::SetSessionConfigOptionRequest,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        Err(acp::Error::method_not_found())
    }
}
