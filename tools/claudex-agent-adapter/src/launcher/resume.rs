use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

const SPAWN_LIMIT_MARKER: &str = "Subagent spawn limit reached (";
const AUTO_FORK_ENV: &str = "CLAUDEX_AUTO_FORK_SPAWN_LIMIT_RESUME";
/// Historical claudex launches injected `--agent claudex-orchestrator`, which
/// Claude Code persists as `agent-setting` and reuses as the session display
/// name on resume. Fresh launches no longer pass that flag; restore a real
/// display name when resuming those contaminated transcripts.
const LEGACY_ORCHESTRATOR_AGENT: &str = "claudex-orchestrator";

pub(super) fn prepare_arguments(arguments: Vec<OsString>, cwd: &Path) -> Vec<OsString> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    prepare_arguments_with_home(
        arguments,
        cwd,
        home.as_deref(),
        config_dir.as_deref(),
        auto_fork_enabled(),
    )
}

fn prepare_arguments_with_home(
    mut arguments: Vec<OsString>,
    cwd: &Path,
    home: Option<&Path>,
    config_dir: Option<&Path>,
    auto_fork: bool,
) -> Vec<OsString> {
    if auto_fork
        && resume_session_id(&arguments).is_some()
        && !has_fork_session(&arguments)
        && home.is_some_and(|home| transcript_has_spawn_limit(home, config_dir, cwd, &arguments))
    {
        arguments.push(OsString::from("--fork-session"));
        eprintln!(
            "claudex: resume history reached Claude Code's subagent limit; continuing with --fork-session"
        );
    }
    if let Some(name) =
        home.and_then(|home| legacy_orchestrator_display_name(home, config_dir, cwd, &arguments))
    {
        arguments.push(OsString::from("--name"));
        arguments.push(OsString::from(name.clone()));
        eprintln!(
            "claudex: restored session display name `{name}` (legacy agent-setting was {LEGACY_ORCHESTRATOR_AGENT})"
        );
    }
    arguments
}

pub(super) fn session_id_for_launch(
    arguments: &[OsString],
    random_id: impl FnOnce() -> String,
) -> String {
    if has_fork_session(arguments) {
        return random_id();
    }
    resume_session_id(arguments).unwrap_or_else(random_id)
}

pub(super) fn session_lock_id(arguments: &[OsString]) -> Option<String> {
    (!has_fork_session(arguments))
        .then(|| resume_session_id(arguments))
        .flatten()
}

fn auto_fork_enabled() -> bool {
    std::env::var(AUTO_FORK_ENV)
        .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(true)
}

fn has_fork_session(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == "--fork-session")
}

fn has_session_name(arguments: &[OsString]) -> bool {
    has_flag(arguments, &["--name", "-n"])
}

fn has_agent(arguments: &[OsString]) -> bool {
    has_flag(arguments, &["--agent"])
}

fn has_flag(arguments: &[OsString], flags: &[&str]) -> bool {
    arguments.iter().enumerate().any(|(index, argument)| {
        argument
            .to_str()
            .is_some_and(|argument| flag_present(arguments, index, argument, flags))
    })
}

fn flag_present(arguments: &[OsString], index: usize, argument: &str, flags: &[&str]) -> bool {
    flags
        .iter()
        .any(|flag| matches_flag(arguments, index, argument, flag))
}

fn matches_flag(arguments: &[OsString], index: usize, argument: &str, flag: &str) -> bool {
    if argument == flag {
        return arguments
            .get(index + 1)
            .and_then(|value| value.to_str())
            .is_some_and(|value| !value.is_empty() && !value.starts_with('-'));
    }
    argument
        .strip_prefix(&format!("{flag}="))
        .is_some_and(|value| !value.is_empty())
}

fn resume_session_id(arguments: &[OsString]) -> Option<String> {
    for (index, argument) in arguments.iter().enumerate() {
        let argument = argument.to_str()?;
        if let Some(value) = argument
            .strip_prefix("--resume=")
            .or_else(|| argument.strip_prefix("-r="))
        {
            return nonempty_resume_id(value);
        }
        if matches!(argument, "--resume" | "-r") {
            return arguments
                .get(index + 1)
                .and_then(|value| value.to_str())
                .and_then(nonempty_resume_id);
        }
    }
    None
}

fn nonempty_resume_id(value: &str) -> Option<String> {
    (!value.is_empty() && !value.starts_with('-')).then(|| value.to_owned())
}

fn legacy_orchestrator_display_name(
    home: &Path,
    config_dir: Option<&Path>,
    cwd: &Path,
    arguments: &[OsString],
) -> Option<String> {
    if has_session_name(arguments) || has_agent(arguments) {
        return None;
    }
    let session_id = resume_session_id(arguments)?;
    let identity = read_transcript_identity(home, config_dir, cwd, &session_id)?;
    if identity.agent_setting.as_deref() != Some(LEGACY_ORCHESTRATOR_AGENT) {
        return None;
    }
    if identity
        .custom_title
        .as_deref()
        .is_some_and(usable_display_name)
    {
        return None;
    }
    identity
        .agent_name
        .filter(|name| usable_display_name(name))
        .or_else(|| identity.slug.filter(|name| usable_display_name(name)))
        .or_else(|| fallback_session_name(cwd, &session_id))
}

fn usable_display_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed != LEGACY_ORCHESTRATOR_AGENT
}

fn fallback_session_name(cwd: &Path, session_id: &str) -> Option<String> {
    cwd.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| usable_display_name(name))
        .map(str::to_owned)
        .or_else(|| {
            let short = session_id.chars().take(8).collect::<String>();
            usable_display_name(&short).then_some(short)
        })
}

#[derive(Debug, Default)]
struct TranscriptIdentity {
    agent_setting: Option<String>,
    custom_title: Option<String>,
    agent_name: Option<String>,
    slug: Option<String>,
}

fn read_transcript_identity(
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

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn transcript_has_spawn_limit(
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

fn transcript_path(
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

fn project_dir_name(cwd: &Path) -> String {
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

#[cfg(test)]
include!("resume_tests.rs");
