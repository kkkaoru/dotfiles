use super::{MessagesRequest, SignaturePool, MAX_SIGNATURE_BUCKETS};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};


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
