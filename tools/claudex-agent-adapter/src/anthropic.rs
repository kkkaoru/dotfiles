mod active_subagent_models;
mod agent_batch;
mod agent_effort;
mod agent_effort_matching;
mod agent_intent_store;
mod agent_route_validation;
mod agent_routing;
mod async_agent_handoff;
pub(crate) use async_agent_handoff::{agent_tool_round_ids, exact_async_launch_acknowledgement};
mod config;
mod content;
mod content_batch;
mod content_pending;
mod error;
mod exhausted_subagent;
mod health;
mod internal_notification;
mod message_router;
mod message_router_dispatch;
mod model_concurrency;
mod pasted_text;
mod request_identity;
mod request_routing;
pub(crate) use request_routing::official_claude_haiku_model;
mod retention;
mod routing_quota;
mod segment;
mod session;
mod stream;
mod stream_batch;
mod subagent_continuation;
mod subagent_reuse;
mod subagent_timeout;
mod task_ids;
// Runtime/daemon option plumbing imports these names once normalized CLI
// configuration is installed on the Bridge. Keep the shared literals available
// while individual library test targets compile without that plumbing.
#[allow(unused_imports)]
pub(crate) use subagent_timeout::{
    LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV, SUBAGENT_HARD_TIMEOUT_ENV,
};
mod provider_auth;
mod provider_auth_cooldown;
mod subscription;
mod subscription_activity;
mod subscription_frames;
mod subscription_oauth;
pub(crate) mod subscription_request;
mod subscription_stream;
mod team_protocol;
mod tool_schema_cache;
mod turn_input;
mod usage_limit_cooldown;
mod usage_limit_failover;

pub use content::{error_response, token_count};
pub use request_identity::RequestIdentity;
use segment::{Segment, Usage, WebEvidenceSummary};
pub(crate) use subscription::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES};

mod bridge_helpers;
mod bridge_instructions;
use bridge_helpers::{intern_signature, trace_request};
use bridge_instructions::{
    BRIDGE_INSTRUCTIONS, CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS, SUBAGENT_RESULT_PROTOCOL,
};

mod bridge_types;
pub use bridge_types::{Bridge, MessagesRequest};
pub(crate) use bridge_types::{
    ActiveTurn, AgentEffortRecord, ContextRetry, MAX_SESSIONS, MAX_SIGNATURE_BUCKETS, SelectedSession,
    Session, SignaturePool,
};

mod bridge_ctors;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod subscription_tests;
#[cfg(test)]
mod tests;
