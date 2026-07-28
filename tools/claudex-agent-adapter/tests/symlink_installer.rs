use std::{fs, os::unix::fs::symlink, path::Path, process::Command};

#[test]
fn installs_current_claudex_agents_and_prunes_renamed_links() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let home = tempfile::tempdir().expect("temporary home");
    let agents = home.path().join(".claude/agents");
    fs::create_dir_all(&agents).expect("temporary agents directory");
    symlink(
        root.join(".claude/agents/claudex-gpt.md"),
        agents.join("claudex-gpt.md"),
    )
    .expect("stale managed agent link");

    let output = Command::new("bash")
        .arg(root.join("create-symlinks.sh"))
        .current_dir(&root)
        .env("HOME", home.path())
        .output()
        .expect("run symlink installer");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!agents.join("claudex-gpt.md").exists());
    for agent in ["claudex-gpt-spark.md", "claudex-ollama-glm-5-2.md"] {
        let installed = agents.join(agent);
        assert!(installed.is_symlink(), "{agent} must be installed");
        assert!(
            installed.exists(),
            "{agent} must target a current definition"
        );
    }
}
