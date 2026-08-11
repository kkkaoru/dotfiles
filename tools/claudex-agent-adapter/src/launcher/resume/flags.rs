use std::ffi::OsString;

use super::{
    AUTO_FORK_ENV,
};

pub(super) fn auto_fork_enabled() -> bool {
    std::env::var(AUTO_FORK_ENV)
        .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(true)
}

pub(super) fn has_fork_session(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == "--fork-session")
}

pub(super) fn has_session_name(arguments: &[OsString]) -> bool {
    has_flag(arguments, &["--name", "-n"])
}

pub(super) fn has_agent(arguments: &[OsString]) -> bool {
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
