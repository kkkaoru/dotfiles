use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

mod dotenv;
use dotenv::{dotenv_fallbacks, dotenv_paths, quoted_value, valid_environment_name};
#[cfg(test)]
#[allow(unused_imports)]
use dotenv::{dotenv_values, DOTENV_FILE_NAME};

const MODEL_PROVIDERS_ROOT_SECTION: &str = "[model_providers]";
const MODEL_PROVIDER_SECTION_PREFIX: &str = "[model_providers.";
const PROVIDER_ENV_KEY: &str = "env_key";

/// Add only provider credentials absent from the daemon environment.
///
/// The app-server process otherwise inherits its parent's environment unchanged. Restricting
/// dotenv loading to `env_key` names declared by copied model-provider configuration avoids
/// forwarding unrelated secrets from either dotenv file.
pub(super) fn credentials(source_home: &Path, isolated_home: &Path) -> HashMap<String, OsString> {
    let Ok(config) = std::fs::read_to_string(isolated_home.join("config.toml")) else {
        return HashMap::new();
    };
    let required = provider_environment_keys(&config);
    if required.is_empty() {
        return HashMap::new();
    }
    let inherited = inherited_environment_keys(&required, |key| std::env::var_os(key));
    let files = dotenv_paths(source_home, std::env::var_os("HOME").map(PathBuf::from));
    dotenv_fallbacks(&required, &inherited, &files)
}

fn inherited_environment_keys(
    required: &HashSet<String>,
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> HashSet<String> {
    required
        .iter()
        .filter(|key| lookup(key).is_some_and(|value| !value.is_empty()))
        .cloned()
        .collect()
}

fn provider_environment_keys(config: &str) -> HashSet<String> {
    let mut in_provider_section = false;
    let mut keys = HashSet::new();
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_provider_section = line == MODEL_PROVIDERS_ROOT_SECTION
                || line.starts_with(MODEL_PROVIDER_SECTION_PREFIX);
            continue;
        }
        if !in_provider_section {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != PROVIDER_ENV_KEY {
            continue;
        }
        if let Some(key) = quoted_value(value.trim()).filter(|key| valid_environment_name(key)) {
            keys.insert(key.to_owned());
        }
    }
    keys
}

#[cfg(test)]
// Coverage gates measure production credentials handling; this inline module only contains tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_valid_model_provider_environment_keys() {
        let config = r#"
env_key = "OUTSIDE"
[model_providers.sakana]
env_key = "SAKANA_KEY"
[model_providers.invalid]
env_key = "invalid-key"
[mcp_servers.unrelated]
env_key = "MCP_KEY"
"#;
        assert_eq!(
            provider_environment_keys(config),
            HashSet::from(["SAKANA_KEY".to_owned()])
        );
    }

    #[test]
    fn preserves_inherited_values_and_uses_dotenv_priority_for_missing_keys() {
        let root = tempfile::tempdir().expect("dotenv fixture");
        let source = root.path().join("codex");
        std::fs::create_dir(&source).expect("source home");
        let user = root.path().join("user.env");
        let source_file = source.join(DOTENV_FILE_NAME);
        std::fs::write(
            &source_file,
            "PRESENT=must-not-replace\nMISSING=source-priority\nUNRELATED=private\n",
        )
        .expect("source dotenv");
        std::fs::write(&user, "MISSING=user-fallback\nSECOND='from-user'\n").expect("user dotenv");
        let required = HashSet::from([
            "PRESENT".to_owned(),
            "MISSING".to_owned(),
            "SECOND".to_owned(),
        ]);
        let inherited = HashSet::from(["PRESENT".to_owned()]);

        let values = dotenv_fallbacks(&required, &inherited, &[source_file, user]);

        assert!(!values.contains_key("PRESENT"));
        assert!(!values.contains_key("UNRELATED"));
        assert_eq!(
            values.get("MISSING"),
            Some(&OsString::from("source-priority"))
        );
        assert_eq!(values.get("SECOND"), Some(&OsString::from("from-user")));
    }

    #[test]
    fn resolves_a_declared_missing_credential_without_forwarding_other_values() {
        let root = tempfile::tempdir().expect("credential fixture");
        let source = root.path().join("source");
        let isolated = root.path().join("isolated");
        std::fs::create_dir(&source).expect("source home");
        std::fs::create_dir(&isolated).expect("isolated home");
        let key = format!("CLAUDEX_TEST_PROVIDER_CREDENTIAL_{}", std::process::id());
        assert!(std::env::var_os(&key).is_none());
        std::fs::write(
            isolated.join("config.toml"),
            format!("[model_providers.fixture]\nenv_key = \"{key}\"\n"),
        )
        .expect("isolated config");
        std::fs::write(
            source.join(DOTENV_FILE_NAME),
            format!("{key}=fixture-value\nUNRELATED=must-not-forward\n"),
        )
        .expect("source dotenv");

        let values = credentials(&source, &isolated);

        assert_eq!(values.get(&key), Some(&OsString::from("fixture-value")));
        assert!(!values.contains_key("UNRELATED"));
        assert!(credentials(&source, &root.path().join("missing")).is_empty());
    }

    #[test]
    fn treats_an_explicit_empty_environment_value_as_missing() {
        let required = HashSet::from(["EMPTY".to_owned(), "PRESENT".to_owned()]);
        let values = HashMap::from([
            ("EMPTY", OsString::new()),
            ("PRESENT", OsString::from("explicit")),
        ]);
        let inherited = inherited_environment_keys(&required, |key| values.get(key).cloned());
        assert_eq!(inherited, HashSet::from(["PRESENT".to_owned()]));
    }

    #[test]
    fn accepts_export_quotes_and_comments_but_skips_empty_credentials() {
        let required = HashSet::from(["FIRST".to_owned(), "SECOND".to_owned(), "EMPTY".to_owned()]);
        let values = dotenv_values(
            "export FIRST=one # comment\nSECOND=\"two#literal\"\nEMPTY=''\n",
            &required,
        );
        assert_eq!(values.get("FIRST").map(String::as_str), Some("one"));
        assert_eq!(
            values.get("SECOND").map(String::as_str),
            Some("two#literal")
        );
        assert!(!values.contains_key("EMPTY"));
    }

    #[test]
    fn keeps_source_and_user_dotenv_paths_distinct_and_ordered() {
        let source = Path::new("/tmp/codex-home");
        assert_eq!(
            dotenv_paths(source, Some(PathBuf::from("/tmp/user-home"))),
            [
                PathBuf::from("/tmp/codex-home/.env"),
                PathBuf::from("/tmp/user-home/.env")
            ]
        );
        assert_eq!(
            dotenv_paths(source, Some(PathBuf::from("/tmp/codex-home"))),
            [PathBuf::from("/tmp/codex-home/.env")]
        );
        assert_eq!(
            dotenv_paths(source, None),
            [PathBuf::from("/tmp/codex-home/.env")]
        );
    }

    #[test]
    fn covers_malformed_provider_and_dotenv_entries() {
        let config = r#"
[unrelated]
env_key = "NOPE"
[model_providers]
not_an_assignment
name = "wrong"
env_key = bare
[model_providers.valid]
env_key = '_VALID_KEY'
env_key = 'invalid-key'
"#;
        assert_eq!(
            provider_environment_keys(config),
            HashSet::from(["_VALID_KEY".to_owned()])
        );

        let required = HashSet::from(["FIRST".to_owned(), "EMPTY".to_owned()]);
        let values = dotenv_values(
            "\n# comment\nnot-an-assignment\nUNKNOWN=value\nexport FIRST='one'\nEMPTY=\n",
            &required,
        );
        assert_eq!(values.get("FIRST").map(String::as_str), Some("one"));
        assert!(!values.contains_key("EMPTY"));
        assert!(
            dotenv_fallbacks(
                &required,
                &HashSet::new(),
                &[PathBuf::from("/definitely/missing/.env")]
            )
            .is_empty()
        );
        assert!(dotenv_fallbacks(&required, &required, &[]).is_empty());
        assert!(quoted_value("").is_none());
        assert!(quoted_value("bare").is_none());
        assert!(quoted_value("\"unterminated").is_none());
        assert!(valid_environment_name("A1_"));
        assert!(!valid_environment_name("1A"));
        assert!(!valid_environment_name(""));
    }
}
