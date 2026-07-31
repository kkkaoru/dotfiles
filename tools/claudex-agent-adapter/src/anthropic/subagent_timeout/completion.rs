use axum::{body::Body, http::Response};

use super::super::{ActiveTurn, Bridge, Segment, content::anthropic_response};

pub(super) async fn finish(bridge: &Bridge, turn: ActiveTurn, segment: Segment) -> Response<Body> {
    super::super::stream::commit_transcript(&turn.session, turn.extras, &segment).await;
    if turn.detached {
        bridge.finish_detached_session(&turn.session).await;
    }
    anthropic_response(segment, &turn.response_model)
}
