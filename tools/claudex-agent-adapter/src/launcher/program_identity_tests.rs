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
