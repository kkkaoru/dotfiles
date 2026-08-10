use super::events::ProgressEvent;

/// Buffer char-by-char Muse Spark NDJSON into readable TUI chunks.
#[derive(Debug, Default)]
pub struct ProgressCoalescer {
    thought: String,
    message: String,
}

const COALESCE_CHARS: usize = 80;
const IMMEDIATE_CHARS: usize = 8;

impl ProgressCoalescer {
    pub fn push(&mut self, event: ProgressEvent) -> Vec<ProgressEvent> {
        match event {
            ProgressEvent::Thought(text) => {
                let appended = text.chars().count();
                self.thought.push_str(&text);
                if should_flush(&self.thought, appended) {
                    self.take_thought().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            ProgressEvent::Message(text) => {
                let appended = text.chars().count();
                self.message.push_str(&text);
                if should_flush(&self.message, appended) {
                    self.take_message().into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            other => {
                let mut out = self.flush_all();
                out.push(other);
                out
            }
        }
    }

    pub fn finish(&mut self) -> Vec<ProgressEvent> {
        self.flush_all()
    }

    fn flush_all(&mut self) -> Vec<ProgressEvent> {
        let mut out = Vec::new();
        out.extend(self.take_thought());
        out.extend(self.take_message());
        out
    }

    fn take_thought(&mut self) -> Option<ProgressEvent> {
        let text = std::mem::take(&mut self.thought);
        nonempty(text).map(ProgressEvent::Thought)
    }

    fn take_message(&mut self) -> Option<ProgressEvent> {
        let text = std::mem::take(&mut self.message);
        nonempty(text).map(ProgressEvent::Message)
    }
}

fn should_flush(buf: &str, just_appended_chars: usize) -> bool {
    if buf.is_empty() {
        return false;
    }
    just_appended_chars >= IMMEDIATE_CHARS
        || buf.contains('\n')
        || buf.ends_with('。')
        || buf.ends_with('！')
        || buf.ends_with('？')
        || buf.ends_with('.')
        || buf.ends_with('!')
        || buf.ends_with('?')
        || buf.chars().count() >= COALESCE_CHARS
}

/// Skip `finalText` when the same assistant text already streamed live.
pub fn remaining_final_message(result_text: &str, streamed: &str) -> Option<String> {
    let result = result_text.trim();
    if result.is_empty() {
        return None;
    }
    let streamed = streamed.trim();
    if streamed.is_empty() {
        return Some(result.to_owned());
    }
    if streamed == result || streamed.contains(result) {
        return None;
    }
    if let Some(rest) = result.strip_prefix(streamed) {
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        return Some(rest.to_owned());
    }
    Some(result.to_owned())
}

pub fn message_text_from_progress(progress: &[ProgressEvent]) -> String {
    progress
        .iter()
        .filter_map(|event| match event {
            ProgressEvent::Message(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn nonempty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "coalesce_tests.rs"]
mod tests;
