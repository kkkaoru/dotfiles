//! Split ACP thought streams into Claude-like thinking units.
//!
//! Providers often stream `AgentThoughtChunk` as one continuous river. Claude Code
//! surfaces thinking in discrete blocks; we open a new unit on paragraph breaks
//! (`\n\n`) and after tool/status interruptions.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct ThoughtUnits {
    sessions: Mutex<HashMap<String, UnitState>>,
}

#[derive(Default)]
struct UnitState {
    /// Current reasoning summary index for this session.
    index: i64,
    /// Last dispatched text left a unit open (no trailing paragraph break).
    open: bool,
}

impl ThoughtUnits {
    /// Map a raw thought chunk into `(summary_index, text)` pieces.
    pub(super) fn partition(&self, session_id: &str, text: &str) -> Vec<(i64, String)> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut sessions = self.sessions.lock().expect("thought units poisoned");
        let state = sessions.entry(session_id.to_owned()).or_default();
        let mut parts = Vec::new();
        let mut remaining = text;
        while let Some((head, tail)) = remaining.split_once("\n\n") {
            if !head.is_empty() {
                parts.push((state.index, head.to_owned()));
                state.open = false;
                state.index = state.index.saturating_add(1);
            } else if state.open {
                // Bare paragraph break closes the open unit.
                state.open = false;
                state.index = state.index.saturating_add(1);
            }
            remaining = tail;
        }
        if !remaining.is_empty() {
            parts.push((state.index, remaining.to_owned()));
            state.open = true;
        }
        parts
    }

    /// Force the next thought onto a new unit (e.g. after a provider tool card).
    pub(super) fn break_after_interrupt(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().expect("thought units poisoned");
        let state = sessions.entry(session_id.to_owned()).or_default();
        if state.open {
            state.open = false;
            state.index = state.index.saturating_add(1);
        }
    }

    pub(super) fn clear(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("thought units poisoned")
            .remove(session_id);
    }
}

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn keeps_streaming_tokens_in_one_unit() {
        let units = ThoughtUnits::default();
        assert_eq!(units.partition("s", "Hel"), vec![(0, "Hel".into())]);
        assert_eq!(units.partition("s", "lo"), vec![(0, "lo".into())]);
    }

    #[test]
    fn splits_paragraph_breaks_into_units() {
        let units = ThoughtUnits::default();
        assert_eq!(
            units.partition("s", "First unit.\n\nSecond unit"),
            vec![(0, "First unit.".into()), (1, "Second unit".into())]
        );
    }

    #[test]
    fn interrupt_starts_a_new_unit() {
        let units = ThoughtUnits::default();
        assert_eq!(
            units.partition("s", "before tool"),
            vec![(0, "before tool".into())]
        );
        units.break_after_interrupt("s");
        assert_eq!(
            units.partition("s", "after tool"),
            vec![(1, "after tool".into())]
        );
    }

    #[test]
    fn consecutive_paragraphs_across_chunks() {
        let units = ThoughtUnits::default();
        assert_eq!(units.partition("s", "One.\n\n"), vec![(0, "One.".into())]);
        assert_eq!(units.partition("s", "Two."), vec![(1, "Two.".into())]);
    }
}
