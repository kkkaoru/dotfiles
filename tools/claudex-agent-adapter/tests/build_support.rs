use std::fs;

use claudex_app_server_adapter::build_support;

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
    build_support::calculate_build_id(&inputs).expect("audit all Rust files");
    build_support::emit_build_metadata(root.path());
}

#[test]
#[should_panic(expected = "Rust files are limited to 500")]
fn rejects_a_rust_file_over_the_line_limit() {
    let contents = "line\n".repeat(build_support::MAX_RUST_FILE_LINES + 1);
    build_support::enforce_line_limit(std::path::Path::new("large.rs"), contents.as_bytes());
}

#[test]
fn accepts_limit_and_non_newline_terminated_files() {
    let exact = "line\n".repeat(build_support::MAX_RUST_FILE_LINES);
    build_support::enforce_line_limit(std::path::Path::new("exact.rs"), exact.as_bytes());
    build_support::enforce_line_limit(std::path::Path::new("single.rs"), b"line");
    build_support::enforce_line_limit(std::path::Path::new("empty.rs"), b"");
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
        1,
        "review every control-flow macro because Clippy nesting skips macro expansions"
    );
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("create build fixture");
    for file in ["build.rs", "Cargo.toml", "Cargo.lock", "clippy.toml"] {
        fs::write(root.path().join(file), file).expect("write root fixture");
    }
    fs::create_dir_all(root.path().join("src/nested")).expect("create source fixture");
    fs::create_dir(root.path().join("tests")).expect("create tests fixture");
    fs::write(
        root.path().join("src/nested/module.rs"),
        "fn fixture() {}\n",
    )
    .expect("write source fixture");
    fs::write(
        root.path().join("tests/check.rs"),
        "#[test] fn check() {}\n",
    )
    .expect("write test fixture");
    fs::write(root.path().join("tests/ignored.txt"), "ignored").expect("write ignored fixture");
    root
}
