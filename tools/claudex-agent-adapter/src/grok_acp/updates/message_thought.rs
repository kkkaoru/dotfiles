use agent_client_protocol as acp;

use super::{
    AGENT_MESSAGE_METHOD, REASONING_METHOD, ThoughtUnits, ThreadEventDispatcher, dispatch_delta,
};

pub(super) fn dispatch_message(
    events: &ThreadEventDispatcher,
    session_id: &str,
    chunk: acp::ContentChunk,
) {
    if let acp::ContentBlock::Text(text) = chunk.content {
        dispatch_delta(
            events,
            session_id,
            AGENT_MESSAGE_METHOD,
            &format!("{session_id}:message"),
            0,
            &text.text,
        );
    }
}

pub(super) fn dispatch_thought(
    events: &ThreadEventDispatcher,
    thoughts: &ThoughtUnits,
    session_id: &str,
    chunk: acp::ContentChunk,
) {
    let acp::ContentBlock::Text(text) = chunk.content else {
        return;
    };
    if text.text.trim().is_empty() {
        return;
    }
    let item_id = format!("{session_id}:reasoning");
    for (summary_index, piece) in thoughts.partition(session_id, &text.text) {
        dispatch_delta(
            events,
            session_id,
            REASONING_METHOD,
            &item_id,
            summary_index,
            &piece,
        );
    }
}
