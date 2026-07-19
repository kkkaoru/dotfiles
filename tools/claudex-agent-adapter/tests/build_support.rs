use std::fs;

use claudex_agent_adapter::build_support;

#[test]
fn repository_production_files_stay_within_the_line_limit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = build_support::build_inputs(root);
    build_support::calculate_build_id(&files).expect("audit production files");
}

#[test]
fn discovers_sorted_inputs_and_hashes_content_deterministically() {
    let root = fixture();
    let inputs = build_support::build_inputs(root.path());
    assert!(inputs.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(
        inputs
            .iter()
            .any(|path| path.ends_with("src/nested/module.rs"))
    );
    let first = build_support::calculate_build_id(&inputs).expect("first build ID");
    let second = build_support::calculate_build_id(&inputs).expect("second build ID");
    assert_eq!(first, second);
    fs::write(
        root.path().join("src/nested/module.rs"),
        "fn changed() {}\n",
    )
    .expect("change fixture");
    assert_ne!(
        first,
        build_support::calculate_build_id(&inputs).expect("changed build ID")
    );
}

#[test]
fn includes_only_rust_files_from_source_trees() {
    let root = fixture();
    let mut inputs = Vec::new();
    build_support::collect_rust_files(&root.path().join("src"), &mut inputs);
    build_support::collect_rust_files(&root.path().join("tests"), &mut inputs);
    inputs.sort();
    assert!(inputs.iter().any(|path| path.ends_with("tests/check.rs")));
    assert!(
        !inputs
            .iter()
            .any(|path| path.ends_with("tests/ignored.txt"))
    );
    inputs.retain(|path| !build_support::is_test_source(path));
    build_support::calculate_build_id(&inputs).expect("audit production Rust files");
    build_support::emit_build_metadata(root.path());
}

#[test]
fn reports_missing_build_inputs_and_source_directories() {
    let root = tempfile::tempdir().expect("fixture");
    fs::create_dir(root.path().join("src")).expect("source directory");
    let missing = root.path().join("missing.rs");
    assert!(build_support::calculate_build_id(&[missing]).is_err());
    assert!(std::panic::catch_unwind(|| build_support::emit_build_metadata(root.path())).is_err());
    assert!(
        std::panic::catch_unwind(|| {
            let mut files = Vec::new();
            build_support::collect_rust_files(&root.path().join("absent"), &mut files);
        })
        .is_err()
    );
}

#[test]
#[should_panic(expected = "has 401 lines; production Rust files are limited to 400")]
fn rejects_401_line_production_file() {
    let contents = "line\n".repeat(401);
    build_support::enforce_line_limit(std::path::Path::new("large.rs"), contents.as_bytes());
}

#[test]
fn accepts_399_and_400_line_production_files() {
    let below_limit = "line\n".repeat(399);
    let at_limit = "line\n".repeat(400);
    build_support::enforce_line_limit(
        std::path::Path::new("below-limit.rs"),
        below_limit.as_bytes(),
    );
    build_support::enforce_line_limit(std::path::Path::new("at-limit.rs"), at_limit.as_bytes());
}

#[test]
fn accepts_non_newline_terminated_and_empty_files() {
    let at_limit_without_trailing_newline = format!("{}line", "line\n".repeat(399));
    build_support::enforce_line_limit(
        std::path::Path::new("at-limit-no-newline.rs"),
        at_limit_without_trailing_newline.as_bytes(),
    );
    build_support::enforce_line_limit(std::path::Path::new("single.rs"), b"line");
    build_support::enforce_line_limit(std::path::Path::new("empty.rs"), b"");
}

#[test]
fn excludes_only_dedicated_test_sources_from_production_inputs() {
    let root = fixture();
    let inputs = build_support::build_inputs(root.path());
    for test_file in [
        "src/tests.rs",
        "src/nested/helper_tests.rs",
        "src/nested/tests/helper.rs",
    ] {
        assert!(!inputs.iter().any(|path| path.ends_with(test_file)));
    }
    assert!(
        inputs
            .iter()
            .any(|path| path.ends_with("src/test_support.rs"))
    );
    build_support::calculate_build_id(&inputs).expect("large test-only files are excluded");
}

#[test]
fn audits_control_flow_hidden_from_clippy_nesting() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    build_support::collect_rust_files(&root.join("src"), &mut files);
    let sources = files
        .iter()
        .map(|path| fs::read_to_string(path).expect("read production source"))
        .collect::<Vec<_>>();
    assert!(
        sources
            .iter()
            .all(|source| !source.contains("macro_rules!"))
    );
    assert_eq!(
        sources
            .iter()
            .map(|source| source.matches("tokio::select!").count())
            .sum::<usize>(),
        5,
        "review every control-flow macro because Clippy nesting skips macro expansions"
    );
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("create build fixture");
    for file in ["build.rs", "Cargo.toml", "Cargo.lock", "clippy.toml"] {
        fs::write(root.path().join(file), file).expect("write root fixture");
    }
    fs::create_dir_all(root.path().join("src/nested")).expect("create source fixture");
    fs::create_dir(root.path().join("src/nested/tests")).expect("create unit test fixture");
    fs::create_dir(root.path().join("tests")).expect("create tests fixture");
    fs::write(
        root.path().join("src/nested/module.rs"),
        "fn fixture() {}\n",
    )
    .expect("write source fixture");
    fs::write(root.path().join("src/test_support.rs"), "fn support() {}\n")
        .expect("write production lookalike");
    let large_test = "test line\n".repeat(build_support::MAX_RUST_FILE_LINES + 1);
    for file in [
        root.path().join("src/tests.rs"),
        root.path().join("src/nested/helper_tests.rs"),
        root.path().join("src/nested/tests/helper.rs"),
    ] {
        fs::write(file, &large_test).expect("write excluded unit test source");
    }
    fs::write(
        root.path().join("tests/check.rs"),
        "#[test] fn check() {}\n",
    )
    .expect("write test fixture");
    fs::write(root.path().join("tests/ignored.txt"), "ignored").expect("write ignored fixture");
    root
}
