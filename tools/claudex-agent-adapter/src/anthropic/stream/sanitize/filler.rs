pub(in crate::anthropic::stream) fn compact_live_prose(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(super::LIVE_PROSE_CHAR_LIMIT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Cursor/Claude worker filler that collapses Claude Code 2.1 to repeating
/// `Thought for Xs` plus mechanical "Working on your request" lines.
pub(in crate::anthropic::stream) fn is_canned_worker_filler(text: &str) -> bool {
    let lower = normalize_filler_case(text);
    if lower.is_empty() {
        return false;
    }
    lower.starts_with("thought for ")
        || lower.starts_with("nucleating")
        || CANNED_FILLER_NEEDLES
            .iter()
            .any(|needle| lower.contains(needle))
}

/// Drop canned filler lines while keeping blank lines and trailing newlines so
/// multi-delta SubAgent answers do not glue headings onto the next bullet.
pub(in crate::anthropic::stream) fn strip_canned_preserving_structure(delta: &str) -> String {
    delta
        .split_inclusive('\n')
        .filter(|chunk| !is_canned_worker_filler(chunk.trim_end_matches('\n')))
        .collect()
}

pub(super) fn normalize_filler_case(text: &str) -> String {
    text.trim()
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
            other => other,
        })
        .collect::<String>()
        .to_ascii_lowercase()
}

pub(super) const CANNED_FILLER_NEEDLES: &[&str] = &[
    "working on your request",
    "i'll gather what i need",
    "put together the result",
    "continuing with the next step",
    "audit the local ctx",
    "local history is indexed",
    "scanning local history",
    "gathering the records for your report",
    "locating local session evidence",
    "pull the evidence needed",
    "pulling event evidence",
    "pulling the provenance",
    "checking for provider history",
];

pub(in crate::anthropic::stream) fn is_provider_status_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    trimmed.starts_with('▶')
        || trimmed.starts_with('✓')
        || trimmed.starts_with('✗')
        || trimmed.starts_with("… still working")
        || trimmed.starts_with("Claudex is still working")
        // Muse one-liner chrome (`Plan next` / `Plan: drafted`) — not CoT that
        // happens to start with the English verb "Plan the …".
        || is_muse_plan_status_line(trimmed)
        || trimmed.starts_with('●')
        || trimmed.starts_with('◎')
        || trimmed.starts_with('○')
        || lower.starts_with("status:")
        || trimmed.starts_with("SubAgent starting:")
        || trimmed.starts_with("SubAgent started:")
        || trimmed.starts_with("SubAgent finished")
        || trimmed.starts_with("SubAgent completed")
        || trimmed.starts_with("Retrying provider request")
        || trimmed.starts_with("Session mode:")
        || trimmed.starts_with("Session:")
        || trimmed.starts_with("🔎 WebSearch:")
}

fn is_muse_plan_status_line(line: &str) -> bool {
    if line.starts_with("Plan:") {
        return true;
    }
    let Some(rest) = line.strip_prefix("Plan ") else {
        return false;
    };
    // Real CoT: "Plan the migration", "Plan to inspect". Muse chrome is a
    // short imperative like "Plan next" without a sentence object.
    let first = rest.split_whitespace().next().unwrap_or("");
    matches!(first, "next" | "drafted" | "ready" | "done") && line.chars().count() <= 32
}

/// Muse Spark often emits `Status: …Status: …` without newlines. Keep only the last
/// status segment — never the answer lines that may follow on later newlines.
pub(in crate::anthropic::stream) fn latest_worker_status(delta: &str) -> Option<String> {
    let trimmed = delta.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("status:") {
        return None;
    }
    let line = trimmed.lines().rev().find(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("status:")
    })?;
    let line = line.trim();
    let line_lower = line.to_ascii_lowercase();
    let last = line_lower.rmatch_indices("status:").next()?.0;
    let status = compact_live_prose(line[last..].trim());
    Some(format!("{}\n", status.trim_end()))
}

/// Drop prior `Status:` chrome so mid-turn updates replace instead of stacking.
pub(in crate::anthropic::stream) fn strip_worker_status_lines(text: &str) -> String {
    let kept = text
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("status:")
        })
        .collect::<Vec<_>>();
    if kept.iter().all(|line| line.trim().is_empty()) {
        return String::new();
    }
    let joined = kept.join("\n");
    if text.ends_with('\n') && !joined.ends_with('\n') {
        format!("{joined}\n")
    } else {
        joined
    }
}
