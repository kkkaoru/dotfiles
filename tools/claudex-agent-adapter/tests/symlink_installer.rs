use std::{fs, os::unix::fs::symlink, path::Path, process::Command};

#[test]
fn installs_current_claudex_agents_and_prunes_renamed_links() {
    let root = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .expect("repository root");
    let home = tempfile::tempdir().expect("temporary home");
    let agents = home.path().join(".claude/agents");
    fs::create_dir_all(&agents).expect("temporary agents directory");
    let stale_link = agents.join("claudex-gpt-renamed.md");
    symlink(
        root.join(".claude/agents/.claudex-gpt-removed-fixture.md"),
        &stale_link,
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
    assert!(
        !stale_link.is_symlink(),
        "stale managed link must be pruned"
    );
    assert!(
        root.join(".claude/agents/claudex-devin-swe-1-7.md")
            .is_file(),
        "Devin SWE-1.7 must remain a managed agent definition"
    );
    let mut installed = fs::read_dir(&agents)
        .expect("installed agent definitions")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_symlink() && entry.path().exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    installed.sort();
    assert_eq!(
        installed,
        [
            "claudex-antigravity-gemini-3-7-flash.md",
            "claudex-cline-deepseek-flash.md",
            "claudex-command-code-muse-spark-1-2-contributor.md",
            "claudex-cursor-luna.md",
            "claudex-cursor-sol.md",
            "claudex-cursor-terra.md",
            "claudex-cursor.md",
            "claudex-deepseek-flash.md",
            "claudex-deepseek-pro.md",
            "claudex-devin-swe-1-7.md",
            "claudex-fugu.md",
            "claudex-gpt-spark.md",
            "claudex-gpt.md",
            "claudex-grok.md",
            "claudex-haiku-search.md",
            "claudex-haiku.md",
            "claudex-ollama-glm-5-2.md",
            "claudex-opencode-gpt.md",
            "claudex-orchestrator.md",
            "claudex-qwen.md",
            "claudex-sonnet.md",
            "custom-advisor.md",
        ]
    );
}
