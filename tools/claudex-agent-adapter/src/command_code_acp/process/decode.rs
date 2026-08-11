use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use super::MAX_STDOUT_LINE_BYTES;

#[derive(Default)]
pub(super) struct Utf8LineDecoder {
    pending: Vec<u8>,
}

impl Utf8LineDecoder {
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn push_line(&mut self, bytes: &[u8]) -> String {
        let mut line = bytes;
        if line.last() == Some(&b'\n') {
            line = &line[..line.len() - 1];
        }
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(line);
        decode_utf8_with_pending(&mut self.pending, data)
    }

    pub(super) fn flush(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.pending);
        let text = String::from_utf8_lossy(&data).into_owned();
        if text.is_empty() { None } else { Some(text) }
    }
}

fn decode_utf8_with_pending(pending: &mut Vec<u8>, data: Vec<u8>) -> String {
    match std::str::from_utf8(&data) {
        Ok(text) => text.to_owned(),
        Err(err) if err.error_len().is_none() => {
            let valid = err.valid_up_to();
            pending.extend_from_slice(&data[valid..]);
            String::from_utf8_lossy(&data[..valid]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(&data).into_owned(),
    }
}

pub(super) async fn read_stdout_line(
    reader: &mut BufReader<impl AsyncRead + Unpin>,
) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(eof_stdout_buf(buf));
        }
        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            buf.extend_from_slice(&available[..=pos]);
            reader.consume(pos + 1);
            return Ok(Some(buf));
        }
        let take = available
            .len()
            .min(MAX_STDOUT_LINE_BYTES.saturating_sub(buf.len()));
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if buf.len() >= MAX_STDOUT_LINE_BYTES {
            return Ok(Some(buf));
        }
    }
}

fn eof_stdout_buf(buf: Vec<u8>) -> Option<Vec<u8>> {
    if buf.is_empty() { None } else { Some(buf) }
}
