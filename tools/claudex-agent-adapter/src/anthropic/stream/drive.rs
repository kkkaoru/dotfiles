use std::time::Duration;

use super::{SegmentBuilder, StreamSender};
use crate::anthropic::{ActiveTurn, model_concurrency::ModelPermit};

mod run;

pub(super) struct ContextRetryStream {
    pub(super) turn: ActiveTurn,
    pub(super) sender: StreamSender,
    pub(super) error: anyhow::Error,
    pub(super) builder: SegmentBuilder,
    pub(super) model_permit: Option<ModelPermit>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
}

pub(super) struct StreamDriveOptions {
    pub(super) model_permit: Option<ModelPermit>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
    pub(super) timeout: Option<Duration>,
}

fn response_timeout(
    configured: Option<Duration>,
    is_subagent: bool,
    run_in_background: bool,
) -> Option<Duration> {
    (is_subagent && run_in_background)
        .then_some(configured)
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::response_timeout;

    #[test]
    fn only_background_subagents_get_an_explicit_hard_timeout() {
        let configured = Some(Duration::from_secs(7));
        assert_eq!(response_timeout(configured, false, false), None);
        assert_eq!(response_timeout(configured, false, true), None);
        assert_eq!(response_timeout(configured, true, false), None);
        assert_eq!(response_timeout(configured, true, true), configured);
        assert_eq!(response_timeout(None, true, true), None);
    }
}
