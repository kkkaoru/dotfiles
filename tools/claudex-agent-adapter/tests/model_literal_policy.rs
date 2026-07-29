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
                !contains_complete_model_id(line) || is_official_haiku_alias_constant(&path, line),
                "complete model ID in production source {}:{}: {line}",
                path.display(),
                index + 1
            );
            assert!(
                !contains_vendor_prefix_inference(line),
                "vendor model-prefix inference in production source {}:{}: {line}",
                path.display(),
                index + 1
            );
        }
    }
}

/// The official Claude Haiku alias is centrally defined once for child routing.
/// All other complete model IDs remain prohibited in production code.
fn is_official_haiku_alias_constant(path: &Path, line: &str) -> bool {
    path.ends_with("src/anthropic/request_routing/models.rs")
        && line.trim() == r#"pub(super) const CLAUDE_HAIKU_MODEL: &str = "claude-haiku-4-5";"#
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

/// Vendor prefixes belong in providers.json (`modelPrefixes`), not production Rust.
/// Matching `starts_with("gpt"|"grok"|"qwen")` hardcodes families and becomes debt on new models.
fn contains_vendor_prefix_inference(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    // Allow comments that document the ban; flag executable-looking inference only.
    if lower.trim_start().starts_with("//") || lower.trim_start().starts_with('#') {
        return false;
    }
    [
        "starts_with(\"gpt\")",
        "starts_with(\"grok\")",
        "starts_with(\"qwen\")",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || [
            "starts_with(\"gpt-\")",
            "starts_with(\"grok-\")",
            "starts_with(\"qwen-\")",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
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
    for rule in [
        "starts_with(\"gpt\")",
        "starts_with(\"grok\")",
        "grok-acp",
        "copilot-acp",
    ] {
        assert!(!contains_complete_model_id(rule), "false positive: {rule}");
    }
}

#[test]
fn allows_only_the_central_official_haiku_alias_constant() {
    let approved = Path::new("src/anthropic/request_routing/models.rs");
    let line = r#"pub(super) const CLAUDE_HAIKU_MODEL: &str = "claude-haiku-4-5";"#;
    assert!(is_official_haiku_alias_constant(approved, line));
    assert!(!is_official_haiku_alias_constant(
        Path::new("src/anthropic/request_routing.rs"),
        line
    ));
    assert!(!is_official_haiku_alias_constant(
        approved,
        r#"pub(super) const CLAUDE_HAIKU_MODEL: &str = "claude-fable-5";"#
    ));
}

#[test]
fn vendor_prefix_inference_detector_flags_hardcoded_families() {
    for line in [
        r#"model.starts_with("gpt") || model.starts_with("grok")"#,
        r#"if model.starts_with("qwen") {"#,
        r#"if model.starts_with("gpt-") {"#,
    ] {
        assert!(
            contains_vendor_prefix_inference(line),
            "missed inference: {line}"
        );
    }
    for line in [
        r#"// Keep off starts_with("gpt") hardcoding"#,
        r"model.starts_with(prefix)",
        r"model.starts_with(prefix.as_str())",
        r#"model.starts_with("vendor-")"#,
    ] {
        assert!(
            !contains_vendor_prefix_inference(line),
            "false positive: {line}"
        );
    }
}
