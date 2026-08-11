use super::{Bridge, MAX_SIGNATURE_BUCKETS, MessagesRequest, Session, SignaturePool};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use crate::agent_backend::AgentBackend;

impl Bridge {
    /// Provider pool for a Claude Code session (isolated Codex/ACP processes).
    pub(in crate::anthropic) fn app_for(
        &self,
        claude_session_id: Option<&str>,
    ) -> Arc<AgentBackend> {
        self.app.scope_or_self(claude_session_id)
    }

    pub(in crate::anthropic) fn app_for_session(&self, session: &Session) -> Arc<AgentBackend> {
        self.app_for(session.claude_session_id.as_deref())
    }

    pub(in crate::anthropic) async fn release_provider_scope_if_unused(
        &self,
        claude_session_id: Option<&str>,
    ) {
        if self.sessions_reference_scope(claude_session_id).await {
            tracing::debug!(
                target: "claudex.provider",
                log_event = "provider_session_scope_retain",
                claude_session_id = claude_session_id.unwrap_or("_anonymous"),
                "keeping Claude-session provider pool while Bridge sessions remain"
            );
            return;
        }
        self.app.release_session_scope(claude_session_id).await;
    }

    async fn sessions_reference_scope(&self, claude_session_id: Option<&str>) -> bool {
        use crate::agent_backend::SessionScopedBackends;
        let key = SessionScopedBackends::scope_key(claude_session_id);
        let matches = |session: &Session| {
            SessionScopedBackends::scope_key(session.claude_session_id.as_deref()) == key
        };
        self.sessions
            .lock()
            .await
            .iter()
            .any(|session| matches(session))
            || self
                .detached_sessions
                .lock()
                .await
                .iter()
                .any(|session| matches(session))
    }
}

pub(super) fn intern_signature(pool: &SignaturePool, signature: String) -> Arc<str> {
    let mut hasher = DefaultHasher::new();
    signature.hash(&mut hasher);
    let mut pool = pool.lock().expect("signature pool poisoned");
    if pool.len() >= MAX_SIGNATURE_BUCKETS {
        pool.retain(|_, candidates| {
            candidates.retain(|candidate| candidate.strong_count() > 0);
            !candidates.is_empty()
        });
    }
    let candidates = pool.entry(hasher.finish()).or_default();
    let mut matched = None;
    candidates.retain(|candidate| {
        let Some(candidate) = candidate.upgrade() else {
            return false;
        };
        if candidate.as_ref() == signature {
            matched = Some(candidate);
        }
        true
    });
    matched.unwrap_or_else(|| {
        let signature = Arc::<str>::from(signature);
        candidates.push(Arc::downgrade(&signature));
        signature
    })
}

pub(super) fn trace_request(request: &MessagesRequest) -> bool {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return false;
    }
    tracing::debug!(
        request_model = %request.model,
        stream = request.stream,
        system_bytes = serialized_len(&request.system),
        message_bytes = serialized_len(&request.messages),
        tool_count = request.tools.len(),
        tool_bytes = serialized_len(&request.tools),
        output_config = %request.output_config,
        "received Claude Code Messages request"
    );
    true
}

fn serialized_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}
