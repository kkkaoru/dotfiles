const SYSTEM_NOTIFICATION_PREFIX: &str = "[SYSTEM NOTIFICATION - NOT USER INPUT]";
const LIFECYCLE_SECTIONS: [(&str, &str); 3] = [
    ("<task-notification", "</task-notification>"),
    ("<agent-message", "</agent-message>"),
    ("<system-reminder", "</system-reminder>"),
];

pub(super) fn classifiable_text(text: &str) -> Option<String> {
    if is_generated_instruction(text) {
        return None;
    }
    let text = sanitize_user_text(text);
    (!text.trim().is_empty()).then_some(text)
}

pub(super) fn is_generated_instruction(text: &str) -> bool {
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("<command-message>")
        || lower.starts_with("<command-name>")
        || lower.starts_with("(re-invocation of /")
        || lower.starts_with("launching skill:")
        || trimmed.starts_with("Base directory for this skill:")
}

fn sanitize_user_text(text: &str) -> String {
    let without_fences = remove_fenced_and_blockquoted_text(text);
    let without_inline_quotes = remove_inline_quoted_text(&without_fences);
    let without_prefixes = remove_system_notification_prefixes(&without_inline_quotes);
    remove_lifecycle_sections(&without_prefixes)
}

pub(super) fn remove_fenced_and_blockquoted_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = fence_marker(trimmed);
        if let Some(marker) = marker {
            update_fence(&mut fence, marker);
            output.push('\n');
            continue;
        }
        if fence.is_some() || trimmed.starts_with('>') {
            output.push('\n');
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn fence_marker(line: &str) -> Option<&'static str> {
    if line.starts_with("```") {
        Some("```")
    } else if line.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

fn update_fence(fence: &mut Option<&'static str>, marker: &'static str) {
    match *fence {
        Some(active) if active == marker => *fence = None,
        None => *fence = Some(marker),
        _ => {}
    }
}

pub(super) fn remove_inline_quoted_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let character = text[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        if let Some(end) = quoted_span_end(text, index, character) {
            index = end;
            continue;
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn quoted_span_end(text: &str, index: usize, opening: char) -> Option<usize> {
    let closing = match opening {
        '"' => '"',
        '“' => '”',
        '「' => '」',
        '『' => '』',
        '`' => '`',
        _ => return None,
    };
    let after_open = index + opening.len_utf8();
    text[after_open..]
        .find(closing)
        .map(|relative_end| after_open + relative_end + closing.len_utf8())
}

fn remove_system_notification_prefixes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(SYSTEM_NOTIFICATION_PREFIX) {
            output.push_str(rest.trim_start());
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    output
}

fn remove_lifecycle_sections(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let next = LIFECYCLE_SECTIONS
            .iter()
            .filter_map(|(opening, closing)| {
                rest.find(opening).map(|start| (start, *opening, *closing))
            })
            .min_by_key(|(start, _, _)| *start);
        let Some((start, opening, closing)) = next else {
            output.push_str(rest);
            return output;
        };
        output.push_str(&rest[..start]);
        let section = &rest[start + opening.len()..];
        let Some(end) = section.find(closing) else {
            return output;
        };
        rest = &section[end + closing.len()..];
        output.push('\n');
    }
}

pub(super) fn remove_negative_or_diagnostic_lines(text: &str) -> String {
    text.lines()
        .map(|line| {
            if is_negated_or_diagnostic(line) {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn is_negated_or_diagnostic(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    [
        "do not ",
        "don't ",
        "dont ",
        "never ",
        "must not ",
        "not a request",
        "disregard ",
        "ignore the instruction",
        "起動しない",
        "起動するな",
        "委譲しない",
        "禁止",
        "不要",
        "誤り",
        "無視して",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || ((lower.starts_with("error:") || lower.starts_with("エラー:"))
            && (lower.contains("worker") || lower.contains("subagent") || lower.contains("launch")))
        || (["wrong", "incorrect", "disproportionate", "tried to push"]
            .iter()
            .any(|marker| lower.contains(marker))
            && (lower.contains("worker")
                || lower.contains("subagent")
                || lower.contains("scope")
                || lower.contains("launch")))
}
