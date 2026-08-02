use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

const SPAWN_LIMIT_MARKER: &str = "Subagent spawn limit reached (";
const AUTO_FORK_ENV: &str = "CLAUDEX_AUTO_FORK_SPAWN_LIMIT_RESUME";

pub(super) fn prepare_arguments(arguments: Vec<OsString>, cwd: &Path) -> Vec<OsString> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    prepare_arguments_with_home(arguments, cwd, home.as_deref(), auto_fork_enabled())
}

fn prepare_arguments_with_home(
    mut arguments: Vec<OsString>,
    cwd: &Path,
    home: Option<&Path>,
    auto_fork: bool,
) -> Vec<OsString> {
    if auto_fork
        && resume_session_id(&arguments).is_some()
        && !has_fork_session(&arguments)
        && home.is_some_and(|home| transcript_has_spawn_limit(home, cwd, &arguments))
    {
        arguments.push(OsString::from("--fork-session"));
        eprintln!(
            "claudex: resume history reached Claude Code's subagent limit; continuing with --fork-session"
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

fn transcript_has_spawn_limit(home: &Path, cwd: &Path, arguments: &[OsString]) -> bool {
    let Some(session_id) = resume_session_id(arguments) else {
        return false;
    };
    let path = transcript_path(home, cwd, &session_id);
    let Ok(file) = File::open(path) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains(SPAWN_LIMIT_MARKER))
}

fn transcript_path(home: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    let project = cwd
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    home.join(".claude/projects")
        .join(project)
        .join(format!("{session_id}.jsonl"))
}

#[cfg(test)]
include!("resume_tests.rs");
