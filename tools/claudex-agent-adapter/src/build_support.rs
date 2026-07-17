use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

pub const MAX_RUST_FILE_LINES: usize = 500;

pub fn emit_build_metadata(root: &Path) {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    let files = build_inputs(root);
    for file in &files {
        println!(
            "cargo:rerun-if-changed={}",
            file.strip_prefix(root).unwrap_or(file).display()
        );
    }
    let build_id = calculate_build_id(&files)
        .unwrap_or_else(|error| panic!("failed to calculate build ID: {error}"));
    println!("cargo:rustc-env=CLAUDEX_BUILD_ID={build_id:016x}");
}

pub fn build_inputs(root: &Path) -> Vec<PathBuf> {
    let mut files = ["build.rs", "Cargo.toml", "Cargo.lock"]
        .map(|file| root.join(file))
        .to_vec();
    collect_rust_files(&root.join("src"), &mut files);
    files.retain(|path| !is_test_source(path));
    files.sort();
    files
}

pub fn is_test_source(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "tests")
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "tests.rs" || name.ends_with("_tests.rs"))
}

pub fn calculate_build_id(files: &[PathBuf]) -> std::io::Result<u64> {
    let mut hasher = DefaultHasher::new();
    for file in files {
        let contents = std::fs::read(file)?;
        if file.extension().is_some_and(|extension| extension == "rs") {
            enforce_line_limit(file, &contents);
        }
        file.hash(&mut hasher);
        contents.hash(&mut hasher);
    }
    Ok(hasher.finish())
}

pub fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        .map(|entry| entry.expect("failed to read source entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

pub fn enforce_line_limit(path: &Path, contents: &[u8]) {
    let line_count = contents.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(contents.last().is_some_and(|byte| *byte != b'\n'));
    assert!(
        line_count <= MAX_RUST_FILE_LINES,
        "{} has {line_count} lines; production Rust files are limited to {MAX_RUST_FILE_LINES}",
        path.display()
    );
}
