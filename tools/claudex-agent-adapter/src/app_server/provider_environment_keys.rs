use std::{collections::HashSet, ffi::OsString};

use super::dotenv::{quoted_value, valid_environment_name};
use super::{MODEL_PROVIDER_SECTION_PREFIX, MODEL_PROVIDERS_ROOT_SECTION, PROVIDER_ENV_KEY};

pub(super) fn inherited_environment_keys(
    required: &HashSet<String>,
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> HashSet<String> {
    required
        .iter()
        .filter(|key| lookup(key).is_some_and(|value| !value.is_empty()))
        .cloned()
        .collect()
}

pub(super) fn provider_environment_keys(config: &str) -> HashSet<String> {
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
