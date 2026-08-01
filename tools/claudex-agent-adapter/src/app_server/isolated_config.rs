use std::collections::HashSet;

use anyhow::Result;

use super::codex_config::provider_config_files;

pub(super) fn append_model_providers(
    source_home: &std::path::Path,
    config: &mut String,
) -> Result<()> {
    let sources = provider_config_files(source_home)?;

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
