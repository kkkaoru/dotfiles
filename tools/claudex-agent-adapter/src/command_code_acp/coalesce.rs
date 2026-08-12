use super::events::ProgressEvent;

/// Buffer char-by-char Muse Spark NDJSON into readable TUI chunks.
#[derive(Debug, Default)]
pub struct ProgressCoalescer {
    thought: String,
    message: String,
    /// True after any Thought was flushed in the current unit. Resets on tools
    /// / status so a later delta-less `thinking_end` can still land.
    thought_emitted: bool,
}

const COALESCE_CHARS: usize = 80;
const IMMEDIATE_CHARS: usize = 8;

impl ProgressCoalescer {
    pub fn push(&mut self, event: ProgressEvent) -> Vec<ProgressEvent> {
        match event {
            ProgressEvent::Thought(text) => self.push_text(true, text),
            ProgressEvent::ThoughtEnd(text) => self.push_thought_end(text),
            ProgressEvent::Message(text) => self.push_text(false, text),
            other => {
                let mut out = self.flush_all();
                self.thought_emitted = false;
                out.push(other);
                out
            }
        }
    }

    fn push_thought_end(&mut self, text: String) -> Vec<ProgressEvent> {
        let mut out: Vec<ProgressEvent> = self.take_thought().into_iter().collect();
        if !out.is_empty() {
            self.thought_emitted = true;
            // Deltas already cover this unit; ignore the full snapshot replay.
            return out;
        }
        if self.thought_emitted || text.trim().is_empty() {
            return out;
        }
        self.thought_emitted = true;
        out.push(ProgressEvent::Thought(text));
        out
    }

    fn push_text(&mut self, thought: bool, text: String) -> Vec<ProgressEvent> {
        let appended = text.chars().count();
        let buf = if thought {
            &mut self.thought
        } else {
            &mut self.message
        };
        buf.push_str(&text);
        if !should_flush(buf, appended) {
            return Vec::new();
        }
        if thought {
            let flushed = self.take_thought().into_iter().collect::<Vec<_>>();
            if !flushed.is_empty() {
                self.thought_emitted = true;
            }
            flushed
        } else {
            self.take_message().into_iter().collect()
        }
    }

    pub fn finish(&mut self) -> Vec<ProgressEvent> {
        self.flush_all()
    }

    fn flush_all(&mut self) -> Vec<ProgressEvent> {
        let mut out = Vec::new();
        if let Some(thought) = self.take_thought() {
            self.thought_emitted = true;
            out.push(thought);
        }
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
