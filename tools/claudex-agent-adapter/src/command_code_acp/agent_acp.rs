use agent_client_protocol as acp;

use super::HeadlessAgent;

impl HeadlessAgent {
    fn initialization_response() -> acp::InitializeResponse {
        acp::InitializeResponse::new(acp::ProtocolVersion::V1)
    }

    fn authentication_response() -> acp::AuthenticateResponse {
        acp::AuthenticateResponse::default()
    }

    fn new_session_response(&self, request: acp::NewSessionRequest) -> acp::NewSessionResponse {
        acp::NewSessionResponse::new(self.open_session(request.cwd))
    }

    async fn prompt_response(
        &self,
        request: acp::PromptRequest,
    ) -> acp::Result<acp::PromptResponse> {
        self.handle_prompt(request).await
    }

    fn cancel_notification(&self, request: &acp::CancelNotification) {
        self.handle_cancel(&request.session_id);
    }

    fn session_model_response() -> acp::SetSessionModelResponse {
        acp::SetSessionModelResponse::default()
    }

    fn session_config_option_response() -> acp::Result<acp::SetSessionConfigOptionResponse> {
        Err(acp::Error::method_not_found())
    }
}

// Nightly branch instrumentation emits an invalid mapping for async-trait's
// generated Agent shim (same llvm-cov getInstantiationGroups crash as Grok's
// ACP client). The inherent handlers above remain instrumented and are covered
// through the fixture's real ACP connection.
// coverage-exception: async-trait-codegen; symbol=impl acp::Agent for HeadlessAgent; evidence=command_code_acp::agent_tests::serve_io_runs_headless_turn_and_emits_tool_progress
#[cfg_attr(coverage_nightly, coverage(off))]
#[async_trait::async_trait(?Send)]
impl acp::Agent for HeadlessAgent {
    async fn initialize(
        &self,
        _request: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        Ok(Self::initialization_response())
    }

    async fn authenticate(
        &self,
        _request: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(Self::authentication_response())
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        Ok(self.new_session_response(request))
    }

    async fn prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        self.prompt_response(request).await
    }

    async fn cancel(&self, request: acp::CancelNotification) -> acp::Result<()> {
        self.cancel_notification(&request);
        Ok(())
    }

    async fn set_session_model(
        &self,
        _request: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        Ok(Self::session_model_response())
    }

    async fn set_session_config_option(
        &self,
        _request: acp::SetSessionConfigOptionRequest,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        Self::session_config_option_response()
    }
}
