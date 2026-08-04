use crate::anthropic::Segment;

use super::SegmentBuilder;

pub(in crate::anthropic) enum StreamTurn {
    Segment {
        segment: Segment,
        provider_settled: bool,
    },
    ContextWindow {
        error: anyhow::Error,
        builder: SegmentBuilder,
    },
    UsageLimit {
        error: anyhow::Error,
        builder: SegmentBuilder,
    },
    ProviderFailure {
        error: anyhow::Error,
    },
    Disconnected,
}
