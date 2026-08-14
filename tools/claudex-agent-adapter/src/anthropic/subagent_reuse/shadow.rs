use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{BuildHasher, BuildHasherDefault, Hash},
    sync::Mutex,
};

use serde_json::Value;

use super::{
    MessagesRequest,
    records::{LaunchRecord, ShadowCandidate, launch_model, shadow_candidate},
};

const KEY_SCHEMA: &str = "idle-shadow-key-v1";

#[derive(Default)]
pub(super) struct ShadowLedger {
    sessions: Mutex<HashMap<u64, ShadowSession>>,
    hasher: BuildHasherDefault<DefaultHasher>,
}

#[derive(Default)]
struct ShadowSession {
    current: CompatibilityKey,
    records: HashMap<u64, CompatibilityKey>,
}

#[derive(Clone, Default, Hash)]
struct CompatibilityKey {
    owner_session: Option<u64>,
    provider: Option<u64>,
    backend: Option<u64>,
    launcher_fingerprint: Option<u64>,
    model: Option<u64>,
    effort: Option<u64>,
    agent_kind: Option<u64>,
    protocol: Option<u64>,
    cwd: Option<u64>,
    mcp_tool_fingerprint: Option<u64>,
    system_fingerprint: Option<u64>,
    git_context_fingerprint: Option<u64>,
    sandbox_fingerprint: Option<u64>,
    auth_generation: Option<u64>,
}

impl CompatibilityKey {
    fn with_model(&self, model: Option<u64>) -> Self {
        let mut key = self.clone();
        key.model = model;
        key
    }
}

impl ShadowLedger {
    pub(super) fn observe_request(
        &self,
        session_id: &str,
        request: &MessagesRequest,
        launches: &[LaunchRecord],
    ) {
        let owner = self.hash_value(&session_id);
        let current = self.request_key(owner, request);
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = sessions.entry(owner).or_default();
        session.current = current.clone();
        for launch in launches {
            let identity = self.record_identity(launch);
            session
                .records
                .entry(identity)
                .or_insert_with(|| current.with_model(self.optional_hash(launch.model.as_deref())));
        }
    }

    pub(super) fn observe_decision(
        &self,
        session_id: &str,
        launches: &[LaunchRecord],
        arguments: &Value,
    ) {
        let owner = self.hash_value(&session_id);
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = sessions.get(&owner) else {
            self.log_unmatched("request_context_unknown");
            return;
        };
        match shadow_candidate(launches, arguments) {
            ShadowCandidate::Selected(launch) => self.observe_selected(session, launch, arguments),
            ShadowCandidate::ScopeUnknown => self.log_unmatched("scope_unknown"),
            ShadowCandidate::NoReusableRecord => self.log_unmatched("no_reusable_record"),
            ShadowCandidate::ScopeMismatch => self.log_unmatched("scope_mismatch"),
        }
    }

    fn request_key(&self, owner_session: u64, request: &MessagesRequest) -> CompatibilityKey {
        let cwd = crate::anthropic::subscription_request::subscription_request_cwd(request)
            .and_then(|path| path.canonicalize().ok())
            .map(|path| self.hash_value(&path));
        CompatibilityKey {
            owner_session: Some(owner_session),
            model: self
                .optional_hash((!request.model.is_empty()).then_some(request.model.as_str())),
            cwd,
            mcp_tool_fingerprint: Some(self.hash_value(&request.tools)),
            system_fingerprint: Some(self.hash_value(&request.system)),
            // These values are not authoritative at the registry boundary.
            // Strict Phase 0 matching treats every unknown as a miss, never a wildcard.
            provider: None,
            backend: None,
            launcher_fingerprint: None,
            effort: None,
            agent_kind: None,
            protocol: None,
            git_context_fingerprint: None,
            sandbox_fingerprint: None,
            auth_generation: None,
        }
    }

    fn record_identity(&self, launch: &LaunchRecord) -> u64 {
        if !launch.key.is_empty() {
            return self.hash_value(&("key", launch.key.as_str()));
        }
        if !launch.recipient.is_empty() {
            return self.hash_value(&("recipient", launch.recipient.as_str()));
        }
        self.hash_value(&(
            "placeholder",
            launch.scope.as_str(),
            launch.model.as_deref(),
        ))
    }

    fn optional_hash(&self, value: Option<&str>) -> Option<u64> {
        value
            .filter(|value| !value.is_empty())
            .map(|value| self.hash_value(&value))
    }

    fn hash_value(&self, value: &impl Hash) -> u64 {
        self.hasher.hash_one(value)
    }

    fn observe_selected(&self, session: &ShadowSession, launch: &LaunchRecord, arguments: &Value) {
        let identity = self.record_identity(launch);
        let Some(recorded) = session.records.get(&identity) else {
            self.log_unmatched("record_key_unknown");
            return;
        };
        let requested = session
            .current
            .with_model(self.optional_hash(launch_model(arguments)));
        let reasons = mismatch_reasons(recorded, &requested);
        // This registry boundary cannot authoritatively resolve every required
        // component yet, so Phase 0 must always report a miss. Match becomes a
        // real outcome only when a later phase supplies every component.
        let key_hash = self.hash_value(&(KEY_SCHEMA, &requested));
        let rss = process_max_rss_bytes();
        tracing::info!(
            target: "claudex_subagent_reuse_shadow",
            phase = "idle_phase0",
            outcome = "miss",
            key_hash = format_args!("{key_hash:016x}"),
            reasons = ?reasons,
            selected_status = launch.status.as_str(),
            process_max_rss_bytes = rss.bytes,
            rss_sample_available = rss.available,
            launch_side_gate_proven = false,
            "observed strict SubAgent reuse key; shadow only, no token savings claimed"
        );
    }

    fn log_unmatched(&self, reason: &'static str) {
        let rss = process_max_rss_bytes();
        tracing::info!(
            target: "claudex_subagent_reuse_shadow",
            phase = "idle_phase0",
            outcome = "miss",
            reasons = ?[reason],
            process_max_rss_bytes = rss.bytes,
            rss_sample_available = rss.available,
            launch_side_gate_proven = false,
            "observed strict SubAgent reuse key; shadow only, no token savings claimed"
        );
    }
}

type ComparedField = (Option<u64>, Option<u64>, &'static str, &'static str);

fn identity_fields(
    recorded: &CompatibilityKey,
    requested: &CompatibilityKey,
) -> [ComparedField; 7] {
    [
        (
            recorded.owner_session,
            requested.owner_session,
            "owner_session_unknown",
            "owner_session_mismatch",
        ),
        (
            recorded.provider,
            requested.provider,
            "provider_unknown",
            "provider_mismatch",
        ),
        (
            recorded.backend,
            requested.backend,
            "backend_unknown",
            "backend_mismatch",
        ),
        (
            recorded.launcher_fingerprint,
            requested.launcher_fingerprint,
            "launcher_fingerprint_unknown",
            "launcher_fingerprint_mismatch",
        ),
        (
            recorded.model,
            requested.model,
            "model_unknown",
            "model_mismatch",
        ),
        (
            recorded.effort,
            requested.effort,
            "effort_unknown",
            "effort_mismatch",
        ),
        (
            recorded.agent_kind,
            requested.agent_kind,
            "agent_kind_unknown",
            "agent_kind_mismatch",
        ),
    ]
}

fn context_fields(recorded: &CompatibilityKey, requested: &CompatibilityKey) -> [ComparedField; 7] {
    [
        (
            recorded.protocol,
            requested.protocol,
            "protocol_unknown",
            "protocol_mismatch",
        ),
        (recorded.cwd, requested.cwd, "cwd_unknown", "cwd_mismatch"),
        (
            recorded.mcp_tool_fingerprint,
            requested.mcp_tool_fingerprint,
            "mcp_tool_fingerprint_unknown",
            "mcp_tool_fingerprint_mismatch",
        ),
        (
            recorded.system_fingerprint,
            requested.system_fingerprint,
            "system_fingerprint_unknown",
            "system_fingerprint_mismatch",
        ),
        (
            recorded.git_context_fingerprint,
            requested.git_context_fingerprint,
            "git_context_fingerprint_unknown",
            "git_context_fingerprint_mismatch",
        ),
        (
            recorded.sandbox_fingerprint,
            requested.sandbox_fingerprint,
            "sandbox_fingerprint_unknown",
            "sandbox_fingerprint_mismatch",
        ),
        (
            recorded.auth_generation,
            requested.auth_generation,
            "auth_generation_unknown",
            "auth_generation_mismatch",
        ),
    ]
}

fn mismatch_reasons(
    recorded: &CompatibilityKey,
    requested: &CompatibilityKey,
) -> Vec<&'static str> {
    identity_fields(recorded, requested)
        .into_iter()
        .chain(context_fields(recorded, requested))
        .filter_map(
            |(recorded, requested, unknown, mismatch)| match (recorded, requested) {
                (Some(left), Some(right)) if left == right => None,
                (Some(_), Some(_)) => Some(mismatch),
                _ => Some(unknown),
            },
        )
        .collect()
}

struct RssSample {
    bytes: u64,
    available: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn process_max_rss_bytes() -> RssSample {
    // Start at zero so a failed syscall remains safe to inspect and is clearly
    // distinguished by `available=false` instead of aborting shadow telemetry.
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    let available = unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } == 0;
    let raw = u64::try_from(usage.ru_maxrss).unwrap_or_default();
    #[cfg(target_os = "linux")]
    let bytes = raw.saturating_mul(1_024);
    #[cfg(target_os = "macos")]
    let bytes = raw;
    RssSample { bytes, available }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_max_rss_bytes() -> RssSample {
    RssSample {
        bytes: 0,
        available: false,
    }
}
