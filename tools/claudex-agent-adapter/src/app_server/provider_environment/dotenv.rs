use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

pub(super) const DOTENV_FILE_NAME: &str = ".env";
const DOTENV_EXPORT_PREFIX: &str = "export ";

pub(super) fn dotenv_paths(source_home: &Path, process_home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = vec![source_home.join(DOTENV_FILE_NAME)];
    match process_home.map(|home| home.join(DOTENV_FILE_NAME)) {
        Some(path) if !paths.contains(&path) => paths.push(path),
        _ => {}
    }
    paths
}

pub(super) fn dotenv_fallbacks(
    required: &HashSet<String>,
    inherited: &HashSet<String>,
    files: &[PathBuf],
) -> HashMap<String, OsString> {
    let missing = required
        .difference(inherited)
        .cloned()
        .collect::<HashSet<_>>();
    if missing.is_empty() {
        return HashMap::new();
    }
    let mut values = HashMap::new();
    for path in files {
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        for (key, value) in dotenv_values(&contents, &missing) {
            values.entry(key).or_insert_with(|| OsString::from(value));
        }
    }
    values
}

pub(super) fn dotenv_values(contents: &str, required: &HashSet<String>) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix(DOTENV_EXPORT_PREFIX).unwrap_or(line);
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !required.contains(key) {
            continue;
        }
        if let Some(value) = dotenv_value(raw_value.trim()).filter(|value| !value.is_empty()) {
            values.insert(key.to_owned(), value);
        }
    }
    values
}

fn dotenv_value(value: &str) -> Option<String> {
    if value.starts_with(['\'', '"']) {
        return quoted_value(value).map(str::to_owned);
    }
    let value = value
        .split_once(" #")
        .map_or(value, |(value, _comment)| value)
        .trim_end();
    Some(value.to_owned())
}

pub(super) fn quoted_value(value: &str) -> Option<&str> {
    let quote = value.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let end = value.as_bytes()[1..]
        .iter()
        .position(|candidate| *candidate == quote)?
        + 1;
    Some(&value[1..end])
}

pub(super) fn valid_environment_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
