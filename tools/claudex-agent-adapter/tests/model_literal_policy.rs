use std::{fs, path::Path};

#[test]
fn production_sources_do_not_pin_complete_model_ids() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rust_sources(&root.join("src"), &mut files);
    files.push(root.join("../../.config/fish/functions/claudex.fish"));

    for path in files {
        let source = fs::read_to_string(&path).expect("read production source");
        let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
        for (index, line) in production.lines().enumerate() {
            assert!(
                !contains_complete_model_id(line),
                "complete model ID in production source {}:{}: {line}",
                path.display(),
                index + 1
            );
        }
    }
}

fn collect_rust_sources(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).expect("read source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && !path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.ends_with("_tests") || stem == "tests")
        {
            files.push(path);
        }
    }
}

fn contains_complete_model_id(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
        .filter(|token| !token.is_empty())
        .any(|token| {
            (token.starts_with("gpt-") && token.bytes().any(|byte| byte.is_ascii_digit()))
                || token.strip_prefix("grok-").is_some_and(|suffix| {
                    suffix.starts_with(|character: char| character.is_ascii_digit())
                })
                || ["claude-sonnet", "claude-opus", "claude-haiku"]
                    .iter()
                    .any(|prefix| token.starts_with(prefix))
                || ["sonnet", "opus", "haiku"].iter().any(|prefix| {
                    token
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.bytes().any(|byte| byte.is_ascii_digit()))
                })
        })
}

#[test]
fn model_literal_detector_covers_provider_and_claude_families() {
    for literal in [
        "gpt-5.example",
        "grok-4.example",
        "claude-sonnet-example",
        "claude-opus-example",
        "claude-haiku-example",
        "sonnet5",
        "opus4.8",
    ] {
        assert!(contains_complete_model_id(literal), "missed {literal}");
    }
    for rule in ["starts_with(\"gpt\")", "starts_with(\"grok\")", "grok-acp"] {
        assert!(!contains_complete_model_id(rule), "false positive: {rule}");
    }
}
