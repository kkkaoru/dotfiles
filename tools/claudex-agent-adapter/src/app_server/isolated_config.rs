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
        append_sections(&contents, config, &mut copied_sections);
    }
    Ok(())
}

fn append_sections(contents: &str, config: &mut String, copied_sections: &mut HashSet<String>) {
    let mut copying = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        copying = next_copying_state(trimmed, copying, copied_sections);
        if copying {
            config.push_str(line);
            config.push('\n');
        }
    }
}

fn next_copying_state(line: &str, current: bool, copied_sections: &mut HashSet<String>) -> bool {
    if !line.starts_with('[') {
        return current;
    }
    let provider_section = line == "[model_providers]" || line.starts_with("[model_providers.");
    provider_section && copied_sections.insert(line.to_owned())
}
