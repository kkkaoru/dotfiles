use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub(in crate::anthropic) fn schema_sort_key(tools: &[Value]) -> Vec<u8> {
    serde_json::to_vec(tools).unwrap_or_default()
}

pub(in crate::anthropic) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
