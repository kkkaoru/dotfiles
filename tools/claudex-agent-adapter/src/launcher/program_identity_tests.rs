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
    fn rejects_non_executables_and_unresolved_backend_programs() {
        let root = tempfile::tempdir().unwrap();
        let regular = root.path().join("regular");
        std::fs::write(&regular, "not executable").unwrap();
        assert!(resolve_program(regular.as_os_str(), OsStr::new("")).is_none());
        assert!(resolve_program(OsStr::new(""), root.path().as_os_str()).is_none());
        assert!(validate(&[BackendRoute::new("missing", BackendKind::PiGateway)]).is_err());
    }

    #[test]
    fn fingerprints_builtin_provider_programs() {
        let routes = [
            BackendRoute::new("codex", BackendKind::CodexAppServer),
            BackendRoute::new("pi", BackendKind::PiGateway),
        ];
        let identity = identity(&routes);
        assert_eq!(identity.programs.len(), routes.len());
    }

    #[test]
    fn program_identity_hash_ignores_ambient_path() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let routes = [BackendRoute::new("gpt", BackendKind::PiGateway)];
        let previous_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", "/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin")
        };
        let first = {
            let mut hasher = DefaultHasher::new();
            identity(&routes).hash(&mut hasher);
            hasher.finish()
        };
        unsafe {
            std::env::set_var(
                "PATH",
                "/tmp/claudex-unused-path:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin",
            )
        };
        let second = {
            let mut hasher = DefaultHasher::new();
            identity(&routes).hash(&mut hasher);
            hasher.finish()
        };
        match previous_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        assert_eq!(first, second);
    }
}
