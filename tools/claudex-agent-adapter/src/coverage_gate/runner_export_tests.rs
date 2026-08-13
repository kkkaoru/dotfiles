use std::fs;

use serde_json::json;

use super::runner::{
    existing_coverage_target, find_profile, has_profile_data, matching_llvm_tool,
    per_object_report, recompute_production_totals,
};

fn write_llvm_cov_target(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let target = root.join("target").join(name);
    fs::create_dir_all(target.join("debug/deps/skip-me")).expect("deps directory");
    fs::write(target.join("merged.profdata"), b"prof").expect("profdata");
    fs::write(target.join("debug/deps/skip-me.rmeta"), b"meta").expect("extension file");
    fs::write(target.join("debug/deps/noext"), b"not-an-object").expect("extensionless file");
    target
}

#[test]
fn profile_discovery_accepts_profdata_and_skips_empty_trees() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    let empty = fixture.path().join("empty");
    fs::create_dir_all(&empty).expect("empty tree");
    assert!(!has_profile_data(&empty));

    let nested = fixture.path().join("nested");
    fs::create_dir_all(nested.join("inner")).expect("nested tree");
    fs::write(nested.join("inner/sample.profraw"), b"raw").expect("profraw");
    assert!(has_profile_data(&nested));

    let missing = fixture.path().join("missing");
    assert!(!has_profile_data(&missing));
}

#[test]
fn existing_target_picks_the_newest_profiled_llvm_cov_dir() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    write_llvm_cov_target(fixture.path(), "llvm-cov-old");
    let newest = write_llvm_cov_target(fixture.path(), "llvm-cov-new");
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(30);
    fs::File::open(newest.parent().unwrap().join("llvm-cov-old"))
        .expect("old target")
        .set_times(fs::FileTimes::new().set_modified(stale))
        .expect("age old target");
    let selected = existing_coverage_target(fixture.path()).expect("profiled target");
    assert_eq!(selected, newest);
    assert!(
        find_profile(&selected)
            .expect("profdata")
            .ends_with("merged.profdata")
    );
}

#[test]
fn per_object_export_skips_directories_and_extension_files() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    let target = write_llvm_cov_target(fixture.path(), "llvm-cov-export");
    let error = per_object_report(&target).expect_err("dummy objects cannot export");
    assert!(
        error.to_string().contains("llvm-cov")
            || error
                .to_string()
                .contains("no per-object llvm-cov exports succeeded"),
        "{error:#}"
    );
}

#[test]
fn matching_llvm_tool_rejects_unknown_binaries() {
    assert!(matching_llvm_tool("llvm-cov-not-a-real-tool").is_none());
    let _ = matching_llvm_tool("llvm-cov");
    let _ = matching_llvm_tool("llvm-profdata");
}

#[test]
fn production_totals_skip_non_production_and_malformed_files() {
    let fixture = tempfile::tempdir().expect("totals fixture");
    let root = fixture.path();
    fs::create_dir_all(root.join("src")).expect("src");
    let mut document = json!({
        "data": [{"files": [
            {
                "filename": format!("{}/src/lib.rs", root.display()),
                "summary": {
                    "lines": {"covered": 3, "count": 4},
                    "functions": {"covered": 1, "count": 1},
                    "regions": {"covered": 2, "count": 2},
                    "branches": {"covered": 1, "count": 2}
                }
            },
            {
                "filename": format!("{}/src/lib_tests.rs", root.display()),
                "summary": {"lines": {"covered": 9, "count": 9}}
            },
            {
                "filename": "/tmp/outside.rs",
                "summary": {"lines": {"covered": 1, "count": 1}}
            },
            {"filename": format!("{}/src/lib.rs", root.display())}
        ]}]
    });
    recompute_production_totals(root, &mut document);
    assert_eq!(document["data"][0]["totals"]["lines"]["covered"], 3);
    assert_eq!(document["data"][0]["totals"]["lines"]["count"], 4);

    let mut missing = json!({"data": [{}]});
    recompute_production_totals(root, &mut missing);
    assert_eq!(missing, json!({"data": [{}]}));
}

#[test]
fn find_profile_reports_when_no_profdata_exists() {
    let fixture = tempfile::tempdir().expect("profile fixture");
    fs::write(fixture.path().join("notes.txt"), b"nope").expect("non-profile");
    let error = find_profile(fixture.path()).expect_err("missing profdata");
    assert!(
        error.to_string().contains("no merged profdata"),
        "{error:#}"
    );
}
