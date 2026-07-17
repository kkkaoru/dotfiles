use std::{ffi::OsStr, fs, path::PathBuf};

use anyhow::{Context, Result};

pub(super) const ROUTING_INSTRUCTIONS: &str = "Grok SubAgent effort routing: when the user \
explicitly requests low, medium, high, or xhigh effort for a SubAgent, use the corresponding \
subagent_type claudex-low, claudex-medium, claudex-high, or claudex-xhigh. This selects only the \
child effort; preserve any separately requested model. Use the normal Grok SubAgent types when \
the user does not specify a child effort.";

const EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

pub(super) fn prepare(program: &OsStr) -> Result<Option<PathBuf>> {
    if let Some(path) = std::env::var_os("CLAUDEX_GROK_PLUGIN_DIR") {
        return Ok(Some(PathBuf::from(path)));
    }
    if PathBuf::from(program).file_name() != Some(OsStr::new("grok")) {
        return Ok(None);
    }
    let home = std::env::var_os("HOME").context("HOME is required for Grok plugin cache")?;
    let root = PathBuf::from(home).join(".cache/claudex/grok-effort-plugin-v1");
    let agents = root.join("agents");
    fs::create_dir_all(&agents).context("create Grok effort plugin cache")?;
    for effort in EFFORTS {
        write_if_changed(
            agents.join(format!("claudex-{effort}.md")),
            &profile(effort),
        )?;
    }
    Ok(Some(root))
}

fn write_if_changed(path: PathBuf, content: &str) -> Result<()> {
    if fs::read_to_string(&path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

fn profile(effort: &str) -> String {
    format!(
        "---\nname: claudex-{effort}\n\
         description: General-purpose SubAgent using {effort} reasoning effort. Use when the user \
         explicitly requests this SubAgent effort.\n\
         promptMode: extend\neffort: {effort}\n---\n\n\
         Work as a general-purpose SubAgent and complete the delegated task.\n"
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{EFFORTS, ROUTING_INSTRUCTIONS, prepare, profile};

    #[test]
    fn profiles_define_every_routable_effort() {
        for effort in EFFORTS {
            let profile = profile(effort);
            assert!(profile.contains(&format!("effort: {effort}")));
            assert!(ROUTING_INSTRUCTIONS.contains(&format!("claudex-{effort}")));
        }
    }

    #[test]
    fn custom_program_does_not_receive_builtin_plugin() {
        assert_eq!(prepare(OsStr::new("grok-acp-mock")).unwrap(), None);
    }
}
