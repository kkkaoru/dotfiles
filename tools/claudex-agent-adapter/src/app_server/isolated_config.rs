use std::{collections::HashSet, path::PathBuf};

use anyhow::Result;

pub(super) fn append_model_providers(
    source_home: &std::path::Path,
    config: &mut String,
) -> Result<()> {
    let mut sources = vec![source_home.join("config.toml")];
    let mut profiles = std::fs::read_dir(source_home)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".config.toml") && name != "config.toml")
        })
        .collect::<Vec<PathBuf>>();
    profiles.sort();
    sources.extend(profiles);

    let mut copied_sections = HashSet::new();
    for source in sources {
        let Ok(contents) = std::fs::read_to_string(source) else {
            continue;
        };
        let mut copying = false;
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                let provider_section =
                    trimmed == "[model_providers]" || trimmed.starts_with("[model_providers.");
                copying = provider_section && copied_sections.insert(trimmed.to_owned());
            }
            if copying {
                config.push_str(line);
                config.push('\n');
            }
        }
    }
    Ok(())
}
