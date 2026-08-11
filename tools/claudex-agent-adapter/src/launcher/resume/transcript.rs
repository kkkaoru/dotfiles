use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use super::{SPAWN_LIMIT_MARKER, resume_session_id};

#[derive(Debug, Default)]
pub(super) struct TranscriptIdentity {
    pub(super) agent_setting: Option<String>,
    pub(super) custom_title: Option<String>,
    pub(super) agent_name: Option<String>,
    pub(super) slug: Option<String>,
}

pub(super) fn read_transcript_identity(
    home: &Path,
    config_dir: Option<&Path>,
    cwd: &Path,
    session_id: &str,
) -> Option<TranscriptIdentity> {
    let path = transcript_path(home, config_dir, cwd, session_id);
    let file = File::open(path).ok()?;
    let mut identity = TranscriptIdentity::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !(line.contains("agent-setting")
            || line.contains("custom-title")
            || line.contains("agent-name")
            || line.contains("\"slug\""))
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match value.get("type").and_then(|value| value.as_str()) {
            Some("agent-setting") => {
                identity.agent_setting = json_string(&value, "agentSetting");
            }
            Some("custom-title") => {
                identity.custom_title = json_string(&value, "customTitle");
            }
            Some("agent-name") => {
                identity.agent_name = json_string(&value, "agentName");
            }
            _ => {}
        }
        if let Some(slug) = json_string(&value, "slug") {
            identity.slug = Some(slug);
        }
    }
    Some(identity)
}

pub(super) fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn transcript_has_spawn_limit(
    home: &Path,
    config_dir: Option<&Path>,
    cwd: &Path,
    arguments: &[OsString],
) -> bool {
    let Some(session_id) = resume_session_id(arguments) else {
        return false;
    };
    let path = transcript_path(home, config_dir, cwd, &session_id);
    let Ok(file) = File::open(path) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains(SPAWN_LIMIT_MARKER))
}

pub(super) fn transcript_path(
    home: &Path,
    config_dir: Option<&Path>,
    cwd: &Path,
    session_id: &str,
) -> PathBuf {
    let project = project_dir_name(cwd);
    let file_name = format!("{session_id}.jsonl");
    if let Some(config_dir) = config_dir {
        let candidate = config_dir.join("projects").join(&project).join(&file_name);
        if candidate.is_file() {
            return candidate;
        }
    }
    home.join(".claude/projects").join(project).join(file_name)
}

pub(super) fn project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}
