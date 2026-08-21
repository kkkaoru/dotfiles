use serde_json::Value;

use super::records::is_send_message_follow_up;

#[cfg(test)]
const URL_PREFIXES: [&str; 2] = ["http://", "https://"];
#[cfg(test)]
const PATH_PREFIXES: [&str; 3] = ["/", "./", "../"];
#[cfg(test)]
const TOKEN_TRIM: &[char] = &['`', '"', '\'', ',', ')', '(', '[', ']', ';'];

pub(in crate::anthropic) fn should_reject_nested_launch(
    is_subagent: bool,
    arguments: &Value,
) -> bool {
    is_subagent && !is_send_message_follow_up(arguments)
}

pub(in crate::anthropic) fn should_reject_live_cap(
    live_count: usize,
    cap: usize,
    arguments: &Value,
) -> bool {
    live_count >= cap && cap > 0 && !is_send_message_follow_up(arguments)
}

#[cfg(test)]
pub(super) fn extract_writer_paths(arguments: &Value) -> Vec<String> {
    let mut paths = extract_paths_from_text(string_field(arguments, "description"));
    paths.extend(extract_paths_from_text(string_field(arguments, "prompt")));
    dedupe_paths(paths)
}

#[cfg(test)]
pub(super) fn extract_paths_from_text(text: &str) -> Vec<String> {
    text.split(is_path_separator)
        .map(normalize_path_token)
        .filter(|token| looks_like_file_path(token))
        .collect()
}

#[cfg(test)]
pub(super) fn paths_overlap(left: &[String], right: &[String]) -> bool {
    left.iter().any(|path| path_hits_any(path, right))
}

#[cfg(test)]
fn path_hits_any(path: &str, others: &[String]) -> bool {
    others.iter().any(|other| path_same_or_nested(path, other))
}

#[cfg(test)]
fn path_same_or_nested(left: &str, right: &str) -> bool {
    left == right
        || is_nested_path(left, right)
        || is_nested_path(right, left)
        || left.ends_with(&format!("/{right}"))
        || right.ends_with(&format!("/{left}"))
}

#[cfg(test)]
fn is_nested_path(parent: &str, child: &str) -> bool {
    child.len() > parent.len()
        && child.as_bytes().get(parent.len()) == Some(&b'/')
        && child.starts_with(parent)
}

#[cfg(test)]
fn string_field<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

#[cfg(test)]
fn is_path_separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '`' | '"' | '\'')
}

#[cfg(test)]
fn normalize_path_token(token: &str) -> String {
    token
        .trim_matches(TOKEN_TRIM)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
const SOURCE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "css", "fish", "go", "h", "html", "java", "js", "json", "jsx", "kt", "md",
    "mjs", "py", "rb", "rs", "sh", "swift", "toml", "ts", "tsx",
];

#[cfg(test)]
fn looks_like_file_path(token: &str) -> bool {
    token.len() >= 3
        && URL_PREFIXES.iter().all(|prefix| !token.starts_with(prefix))
        && (PATH_PREFIXES.iter().any(|prefix| token.starts_with(prefix))
            || (token.contains('/') && token.contains('.'))
            || token.contains('\\')
            || has_source_extension(token))
}

#[cfg(test)]
fn has_source_extension(token: &str) -> bool {
    let Some((stem, extension)) = token.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && SOURCE_EXTENSIONS
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
fn dedupe_paths(paths: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    paths.into_iter().for_each(|path| {
        if !path.is_empty() && !unique.iter().any(|seen| seen == &path) {
            unique.push(path);
        }
    });
    unique
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "enforcement_tests.rs"]
mod tests;
