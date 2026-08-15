use std::{
    ffi::{OsStr, OsString},
    hash::Hash,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::agent_backend::{BackendKind, BackendRoute};

const CODEX_PROGRAM_ENV: &str = "CLAUDEX_CODEX_PROGRAM";
const COPILOT_PROGRAM_ENV: &str = "CLAUDEX_COPILOT_PROGRAM";
const GROK_PROGRAM_ENV: &str = "CLAUDEX_GROK_PROGRAM";
const PI_PROGRAM_ENV: &str = "CLAUDEX_PI_PROGRAM";
const GROK_PLUGIN_DIR_ENV: &str = "CLAUDEX_GROK_PLUGIN_DIR";

#[derive(Debug, Hash)]
pub(super) struct DaemonProgramIdentity {
    programs: Vec<(String, PathBuf)>,
    search_path: OsString,
    grok_plugin_directory: Option<PathBuf>,
}

pub(super) fn identity(routes: &[BackendRoute]) -> DaemonProgramIdentity {
    let search_path = crate::path_env::tool_search_path();
    let programs = routes
        .iter()
        .map(|route| {
            let program = route_program(route);
            let normalized =
                resolve_program(&program, &search_path).unwrap_or_else(|| PathBuf::from(&program));
            (route.model.clone(), normalized)
        })
        .collect();
    let grok_plugin_directory = routes
        .iter()
        .any(|route| route.acp.is_none() && route.backend == BackendKind::GrokAcp)
        .then(|| std::env::var_os(GROK_PLUGIN_DIR_ENV))
        .flatten()
        .map(PathBuf::from)
        .map(|path| path.canonicalize().unwrap_or(path));
    DaemonProgramIdentity {
        programs,
        search_path,
        grok_plugin_directory,
    }
}

pub(super) fn validate(routes: &[BackendRoute]) -> Result<()> {
    let search_path = crate::path_env::tool_search_path();
    for route in routes {
        let program = route_program(route);
        if resolve_program(&program, &search_path).is_none() {
            bail!(
                "adapter preflight cannot resolve backend command `{}` for model `{}`",
                program.to_string_lossy(),
                route.model
            );
        }
    }
    Ok(())
}

fn route_program(route: &BackendRoute) -> OsString {
    if let Some(acp) = &route.acp {
        return OsString::from(&acp.program);
    }
    let (environment, fallback) = match route.backend {
        BackendKind::CodexAppServer => (CODEX_PROGRAM_ENV, "codex"),
        BackendKind::CopilotAcp => (COPILOT_PROGRAM_ENV, "copilot"),
        BackendKind::GrokAcp => (GROK_PROGRAM_ENV, "grok"),
        BackendKind::PiGateway => (PI_PROGRAM_ENV, "pi"),
        BackendKind::ConfiguredAcp => ("", ""),
    };
    if environment.is_empty() {
        return OsString::new();
    }
    std::env::var_os(environment).unwrap_or_else(|| fallback.into())
}

fn resolve_program(program: &OsStr, search_path: &OsStr) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let path = Path::new(program);
    if path.components().count() > 1 {
        return executable(path).then(|| path.canonicalize().unwrap_or_else(|_| path.to_owned()));
    }
    std::env::split_paths(search_path)
        .map(|directory| directory.join(program))
        .find(|candidate| executable(candidate))
        .map(|path| path.canonicalize().unwrap_or(path))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
include!("program_identity_tests.rs");
