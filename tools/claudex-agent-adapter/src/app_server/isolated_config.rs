use anyhow::Result;

pub(super) fn append_model_providers(
    source_home: &std::path::Path,
    config: &mut String,
) -> Result<()> {
    let source = source_home.join("config.toml");
    let Ok(contents) = std::fs::read_to_string(&source) else {
        return Ok(());
    };
    let mut copying = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            copying = trimmed == "[model_providers]" || trimmed.starts_with("[model_providers.");
        }
        if copying {
            config.push_str(line);
            config.push('\n');
        }
    }
    Ok(())
}
