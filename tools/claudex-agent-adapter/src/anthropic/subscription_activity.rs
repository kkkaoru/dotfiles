use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    stream::send_stream_frame,
    subscription_frames::{send_block_stop, send_text_delta},
};

const HEARTBEAT: &str = "\u{200b}";
const STATUS: &str = "Claudex is still working; waiting for provider output\u{2026}";

#[derive(Default)]
pub(super) struct SubscriptionActivity {
    open: Option<OpenActivity>,
}

struct OpenActivity {
    index: usize,
    signature: String,
}

impl SubscriptionActivity {
    pub(super) async fn keepalive(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        text_index: Option<usize>,
        next_index: &mut usize,
    ) -> Result<()> {
        if let Some(index) = text_index {
            return send_text_delta(sender, index, HEARTBEAT).await;
        }
        if let Some(activity) = &self.open {
            return send_thinking_delta(sender, activity.index, HEARTBEAT).await;
        }
        let index = *next_index;
        send_stream_frame(Some(sender), "content_block_start", || {
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            })
        })
        .await?;
        send_thinking_delta(sender, index, STATUS).await?;
        self.open = Some(OpenActivity {
            index,
            signature: format!("claudex_local_{}", Uuid::new_v4().simple()),
        });
        *next_index += 1;
        Ok(())
    }

    pub(super) async fn close(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        let Some(activity) = self.open.take() else {
            return Ok(());
        };
        send_stream_frame(Some(sender), "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":activity.index,
                "delta":{"type":"signature_delta","signature":activity.signature}
            })
        })
        .await?;
        send_block_stop(sender, activity.index).await
    }
}

async fn send_thinking_delta(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    index: usize,
    thinking: &str,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_delta", || {
        json!({
            "type":"content_block_delta", "index":index,
            "delta":{"type":"thinking_delta","thinking":thinking}
        })
    })
    .await
}
