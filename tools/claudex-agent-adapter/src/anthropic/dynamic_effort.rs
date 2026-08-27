use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use super::{
    Bridge, MessagesRequest, request_identity::claude_session_id,
    subscription_request::is_compaction_request, token_count,
};

const RAMP_START_NUMERATOR: u64 = 3;
const RAMP_START_DENOMINATOR: u64 = 5;
const RESET_COMPACTION_INTERVAL: u64 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Effort {
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Default)]
struct SessionState {
    compaction_count: u64,
    compaction_pending: bool,
    force_baseline: bool,
}

#[derive(Debug, Default)]
pub(in crate::anthropic) struct DynamicEffortManager {
    enabled: bool,
    sessions: Mutex<HashMap<String, SessionState>>,
}

impl DynamicEffortManager {
    pub(in crate::anthropic) fn from_environment() -> Self {
        Self {
            enabled: std::env::var("CLAUDEX_DYNAMIC_EFFORT")
                .ok()
                .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "on")),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub(in crate::anthropic) fn resolve(
        &self,
        session_id: Option<&str>,
        context_tokens: u64,
        context_window: Option<u64>,
        is_compaction: bool,
        is_subagent: bool,
        configured: Option<String>,
    ) -> Option<String> {
        if !self.enabled || is_subagent {
            return configured;
        }
        let Some(session_id) = session_id else {
            return configured;
        };
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        let state = sessions.entry(session_id.to_owned()).or_default();
        if is_compaction {
            state.compaction_pending = true;
            return Some(Effort::Max.as_str().to_owned());
        }
        let Some(context_window) = context_window else {
            return configured;
        };
        if state.compaction_pending {
            state.compaction_pending = false;
            state.compaction_count = state.compaction_count.saturating_add(1);
            state.force_baseline = state
                .compaction_count
                .is_multiple_of(RESET_COMPACTION_INTERVAL);
        }
        let effort = select_effort(context_tokens, context_window, state.force_baseline);
        state.force_baseline = false;
        Some(effort.as_str().to_owned())
    }
}

impl Bridge {
    pub(in crate::anthropic) fn resolve_dynamic_effort(
        &self,
        request: &MessagesRequest,
        is_subagent: bool,
        configured: Option<String>,
    ) -> Option<String> {
        if self.app.launch_scoped_effort(&request.model).is_some() {
            return configured;
        }
        let session_id = claude_session_id(request);
        let context_tokens = u64::try_from(token_count(request)).unwrap_or(u64::MAX);
        self.dynamic_effort.resolve(
            session_id.as_deref(),
            context_tokens,
            self.app.max_context_tokens_for_model(&request.model),
            is_compaction_request(request),
            is_subagent,
            configured,
        )
    }
}

fn select_effort(context_tokens: u64, context_window: u64, force_baseline: bool) -> Effort {
    if force_baseline || context_window == 0 {
        return Effort::Medium;
    }
    let ramp_start = context_window.saturating_mul(RAMP_START_NUMERATOR) / RAMP_START_DENOMINATOR;
    if context_tokens <= ramp_start {
        return Effort::Medium;
    }
    let ramp_width = context_window.saturating_sub(ramp_start).max(1);
    let progress = context_tokens.saturating_sub(ramp_start);
    if progress.saturating_mul(3) >= ramp_width.saturating_mul(2) {
        Effort::XHigh
    } else if progress.saturating_mul(3) >= ramp_width {
        Effort::High
    } else {
        Effort::Medium
    }
}

#[cfg(test)]
mod tests {
    use super::{DynamicEffortManager, Effort, SessionState, select_effort};
    use std::{collections::HashMap, sync::Mutex};

    #[test]
    fn ramps_to_the_penultimate_effort() {
        assert_eq!(select_effort(50, 100, false), Effort::Medium);
        assert_eq!(select_effort(61, 100, false), Effort::Medium);
        assert_eq!(select_effort(74, 100, false), Effort::High);
        assert_eq!(select_effort(90, 100, false), Effort::XHigh);
        assert_eq!(select_effort(100, 100, false), Effort::XHigh);
    }

    #[test]
    fn uses_max_for_compaction_and_resets_after_three_compactions() {
        let manager = DynamicEffortManager {
            enabled: true,
            sessions: Mutex::new(HashMap::new()),
        };
        assert_eq!(
            manager.resolve(
                Some("session"),
                90,
                Some(100),
                true,
                false,
                Some("low".to_owned())
            ),
            Some("max".to_owned())
        );
        assert_eq!(
            manager.resolve(Some("session"), 90, Some(100), false, false, None),
            Some("xhigh".to_owned())
        );
        assert_eq!(
            manager.resolve(Some("session"), 90, Some(100), true, false, None),
            Some("max".to_owned())
        );
        assert_eq!(
            manager.resolve(Some("session"), 90, Some(100), false, false, None),
            Some("xhigh".to_owned())
        );
        assert_eq!(
            manager.resolve(Some("session"), 90, Some(100), true, false, None),
            Some("max".to_owned())
        );
        assert_eq!(
            manager.resolve(Some("session"), 90, Some(100), false, false, None),
            Some("medium".to_owned())
        );
    }

    #[test]
    fn preserves_configured_effort_when_dynamic_control_is_inapplicable() {
        let disabled = DynamicEffortManager::default();
        assert_eq!(
            disabled.resolve(
                Some("session"),
                90,
                Some(100),
                false,
                false,
                Some("high".to_owned())
            ),
            Some("high".to_owned())
        );
        let enabled = DynamicEffortManager {
            enabled: true,
            sessions: Mutex::new(HashMap::from([(
                "existing".to_owned(),
                SessionState::default(),
            )])),
        };
        assert_eq!(
            enabled.resolve(None, 90, Some(100), false, false, Some("high".to_owned())),
            Some("high".to_owned())
        );
        assert_eq!(
            enabled.resolve(
                Some("session"),
                90,
                None,
                false,
                false,
                Some("high".to_owned())
            ),
            Some("high".to_owned())
        );
        assert_eq!(
            enabled.resolve(
                Some("session"),
                90,
                Some(100),
                false,
                true,
                Some("high".to_owned())
            ),
            Some("high".to_owned())
        );
        assert_eq!(select_effort(90, 0, false), Effort::Medium);
        assert_eq!(select_effort(90, 100, true), Effort::Medium);
    }
}
