use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

mod dotenv;
#[cfg(test)]
use std::collections::HashSet;
#[cfg(test)]
#[allow(unused_imports)]
use dotenv::{DOTENV_FILE_NAME, dotenv_values, quoted_value, valid_environment_name};
use dotenv::{dotenv_fallbacks, dotenv_paths};

pub(super) const MODEL_PROVIDERS_ROOT_SECTION: &str = "[model_providers]";
pub(super) const MODEL_PROVIDER_SECTION_PREFIX: &str = "[model_providers.";
pub(super) const PROVIDER_ENV_KEY: &str = "env_key";

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

#[path = "provider_environment_keys.rs"]
mod keys;
use keys::{inherited_environment_keys, provider_environment_keys};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "provider_environment_tests.rs"]
mod tests;
