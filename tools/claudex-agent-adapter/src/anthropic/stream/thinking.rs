use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{StreamSender, send_stream_frame};

#[derive(Default)]
pub(super) struct ThinkingState {
    open: Option<OpenThinking>,
}

struct OpenThinking {
    index: usize,
    item_id: String,
    summary_index: i64,
    signature: String,
    text: String,
}

impl ThinkingState {
    pub(super) async fn delta(
        &mut self,
        event: &Value,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some((item_id, summary_index, delta)) = summary_delta(event) else {
            return Ok(());
        };
        if delta.is_empty() || has_visible_output(blocks) {
            return Ok(());
        }
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.item_id != item_id)
        {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, item_id, summary_index, stream).await?;
        }
        let open = self.open.as_mut().expect("thinking block just opened");
        let separator = if open.summary_index != summary_index {
            "\n\n"
        } else {
            ""
        };
        open.summary_index = summary_index;
        open.text.push_str(separator);
        open.text.push_str(delta);
        send_stream_frame(
            stream,
            "content_block_delta",
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"thinking_delta","thinking":format!("{separator}{delta}")}
            }),
        )
        .await
    }

    async fn start(
        &mut self,
        blocks: &mut Vec<Value>,
        item_id: &str,
        summary_index: i64,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let index = blocks.len();
        blocks.push(json!({"type":"thinking","thinking":"","signature":""}));
        send_stream_frame(
            stream,
            "content_block_start",
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            }),
        )
        .await?;
        self.open = Some(OpenThinking {
            index,
            item_id: item_id.to_owned(),
            summary_index,
            signature: format!("claudex_local_{}", Uuid::new_v4().simple()),
            text: String::new(),
        });
        Ok(())
    }

    pub(super) async fn close(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        blocks[open.index]["thinking"] = json!(open.text);
        blocks[open.index]["signature"] = json!(open.signature);
        send_stream_frame(
            stream,
            "content_block_delta",
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"signature_delta","signature":blocks[open.index]["signature"]}
            }),
        )
        .await?;
        send_stream_frame(
            stream,
            "content_block_stop",
            json!({"type":"content_block_stop","index":open.index}),
        )
        .await
    }
}

fn summary_delta(event: &Value) -> Option<(&str, i64, &str)> {
    let params = event.get("params")?;
    Some((
        params.get("itemId")?.as_str()?,
        params.get("summaryIndex")?.as_i64()?,
        params.get("delta")?.as_str()?,
    ))
}

fn has_visible_output(blocks: &[Value]) -> bool {
    blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) != Some("thinking"))
}
