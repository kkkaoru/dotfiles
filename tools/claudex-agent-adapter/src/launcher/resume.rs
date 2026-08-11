mod flags;
mod transcript;
use flags::{auto_fork_enabled, has_agent, has_fork_session, has_session_name};
#[cfg(test)]
use transcript::project_dir_name;
use transcript::{read_transcript_identity, transcript_has_spawn_limit};

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const SPAWN_LIMIT_MARKER: &str = "Subagent spawn limit reached (";
pub(super) const AUTO_FORK_ENV: &str = "CLAUDEX_AUTO_FORK_SPAWN_LIMIT_RESUME";
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

pub(super) fn resume_session_id(arguments: &[OsString]) -> Option<String> {
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

#[cfg(test)]
include!("resume_tests.rs");
