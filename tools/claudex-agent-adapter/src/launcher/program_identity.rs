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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn resolves_commands_with_the_daemon_search_path() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("provider");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(
            resolve_program(OsStr::new("provider"), root.path().as_os_str()),
            Some(program.canonicalize().unwrap())
        );
        assert!(resolve_program(OsStr::new("missing"), root.path().as_os_str()).is_none());
    }

    #[test]
    fn rejects_non_executables_and_preserves_explicit_acp_program_identity() {
        let root = tempfile::tempdir().unwrap();
        let regular = root.path().join("regular");
        std::fs::write(&regular, "not executable").unwrap();
        assert!(resolve_program(regular.as_os_str(), OsStr::new("")).is_none());
        assert!(resolve_program(OsStr::new(""), root.path().as_os_str()).is_none());

        let explicit = root.path().join("provider");
        std::fs::write(&explicit, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&explicit, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut route = BackendRoute::new("explicit", BackendKind::ConfiguredAcp);
        route.acp = Some(crate::agent_backend::AcpLaunch {
            program: explicit.to_string_lossy().into_owned(),
            arguments: vec!["--stdio".to_owned()],
        });
        let identity = identity(&[route]);
        assert_eq!(identity.programs[0].1, explicit.canonicalize().unwrap());
        assert!(validate(&[BackendRoute::new("missing", BackendKind::ConfiguredAcp)]).is_err());
    }

    #[test]
    fn fingerprints_builtin_provider_programs_and_grok_plugin_directory() {
        let root = tempfile::tempdir().unwrap();
        let plugin = root.path().join("grok-plugin");
        std::fs::create_dir(&plugin).unwrap();
        let previous = std::env::var_os(GROK_PLUGIN_DIR_ENV);
        unsafe { std::env::set_var(GROK_PLUGIN_DIR_ENV, &plugin) };
        let routes = [
            BackendRoute::new("codex", BackendKind::CodexAppServer),
            BackendRoute::new("copilot", BackendKind::CopilotAcp),
            BackendRoute::new("grok", BackendKind::GrokAcp),
            BackendRoute::new("configured", BackendKind::ConfiguredAcp),
        ];
        let identity = identity(&routes);
        assert_eq!(
            identity.grok_plugin_directory,
            Some(plugin.canonicalize().unwrap())
        );
        assert_eq!(identity.programs.len(), routes.len());
        match previous {
            Some(value) => unsafe { std::env::set_var(GROK_PLUGIN_DIR_ENV, value) },
            None => unsafe { std::env::remove_var(GROK_PLUGIN_DIR_ENV) },
        }
    }
}
