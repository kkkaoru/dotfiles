use std::io::{self, Write};

use serde::Serialize;
use serde_json::Value;

pub(super) fn encoded_string_content_bytes(value: &str) -> usize {
    encoded_bytes(value).saturating_sub(2)
}

pub(super) fn event_bytes(event: &Value) -> usize {
    encoded_bytes(event)
}

fn encoded_bytes(value: &(impl Serialize + ?Sized)) -> usize {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_or(usize::MAX, |()| counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::Write as _;

    use super::ByteCounter;

    #[test]
    fn flushes_a_byte_counter_without_changing_its_count() {
        let mut counter = ByteCounter::default();
        assert_eq!(counter.write(b"payload").expect("count bytes"), 7);
        counter.flush().expect("flush is a no-op");
        assert_eq!(counter.bytes, 7);
    }
}
