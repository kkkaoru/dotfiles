use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    process::Command,
};

use serde_json::{Value, json};

use super::runner::{
    command_status, coverage_arguments, coverage_command, coverage_target_directory,
    discard_successful_artifacts, prune_stale_coverage_artifacts, report_arguments, run_commands,
    run_with, should_retry_llvm_cov_export, shrink_failed_coverage_target,
};
use super::{
    INSTRUMENTATION_EXCEPTIONS, audit_report, combine_object_reports, coverage_percent,
    is_non_executable_source, is_test_only_source, report, source_branch_percent,
    source_line_percent,
};

#[test]
fn coverage_command_includes_branch_and_build_script_measurement() {
    let arguments = coverage_arguments(std::path::Path::new("report.json"));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--branch", "--all-targets"])
    );
    assert!(arguments.contains(&"--include-build-script".to_owned()));
    assert!(arguments.contains(&"/tests/fixtures/".to_owned()));
    assert!(!arguments.contains(&"--summary-only".to_owned()));
    assert_eq!(arguments.last().map(String::as_str), Some("report.json"));
}

#[test]
fn retry_policy_only_retries_json_segfaults() {
    assert!(!should_retry_llvm_cov_export(
        &[],
        std::process::ExitStatus::from_raw(0)
    ));
    assert!(!should_retry_llvm_cov_export(
        &["--json".to_owned()],
        std::process::ExitStatus::from_raw(1)
    ));
    assert!(should_retry_llvm_cov_export(
        &["--json".to_owned()],
        std::process::ExitStatus::from_raw(libc::SIGSEGV)
    ));
}

#[test]
fn object_combiner_sums_duplicate_branch_counts_without_new_denominators() {
    let first = json!({"data": [{"files": [{
        "filename": "src/example.rs",
        "branches": [[1, 1, 1, 2, 2, 0, 0, 0, 4]]
    }]}]});
    let second = json!({"data": [{"files": [{
        "filename": "src/example.rs",
        "branches": [[1, 1, 1, 2, 0, 3, 0, 0, 4]]
    }]}]});
    let combined = combine_object_reports(&[first, second]);
    let branches = &combined["data"][0]["files"][0]["branches"];
    assert_eq!(branches.as_array().expect("branches").len(), 1);
    assert_eq!(branches[0][4], 2);
    assert_eq!(branches[0][5], 3);
}

#[test]
fn combiner_skips_reports_without_file_arrays_or_filenames() {
    let combined = combine_object_reports(&[
        json!({}),
        json!({"data": [{}]}),
        json!({"data": [{"files": "nope"}]}),
        json!({"data": [{"files": [{}, {"filename": 1}]}]}),
    ]);
    assert_eq!(
        combined["data"][0]["files"].as_array().map(Vec::len),
        Some(0)
    );
}

fn summary_metric(count: u64, covered: u64) -> Value {
    json!({"count": count, "covered": covered})
}

#[test]
fn combiner_takes_higher_summary_counts_and_skips_invalid_branch_records() {
    let first = json!({"data": [{"files": [{
        "filename": "src/a.rs",
        "summary": {
            "lines": summary_metric(1, 1),
            "functions": summary_metric(10, 8),
            "regions": summary_metric(4, 4)
        },
        "branches": [[1, 1, 1, 1, 1, 0, 0, 0, 4], "skip", []]
    }]}]});
    let second = json!({"data": [{"files": [{
        "filename": "src/a.rs",
        "summary": {
            "lines": summary_metric(5, 5),
            "functions": summary_metric(3, 2),
            "regions": summary_metric(4, 1)
        },
        "branches": [[1, 1, 1, 1, 0, 1, 0, 0, 4]]
    }]}]});
    let file = &combine_object_reports(&[first, second])["data"][0]["files"][0];
    assert_eq!(file["summary"]["lines"]["count"], 5);
    assert_eq!(file["summary"]["lines"]["covered"], 5);
    assert_eq!(file["summary"]["functions"]["count"], 10);
    assert_eq!(file["summary"]["functions"]["covered"], 8);
    assert_eq!(file["summary"]["regions"]["covered"], 4);
    assert_eq!(file["branches"].as_array().expect("branches").len(), 1);
    assert_eq!(file["branches"][0][4], 1);
    assert_eq!(file["branches"][0][5], 1);
}

#[test]
fn combiner_keeps_existing_branches_when_the_other_side_has_none() {
    let with_branches = json!({"data": [{"files": [{
        "filename": "src/a.rs",
        "branches": [[1, 1, 1, 1, 1, 1, 0, 0, 4]]
    }]}]});
    let without = json!({"data": [{"files": [{
        "filename": "src/a.rs",
        "summary": {"lines": {"count": 1, "covered": 1}}
    }]}]});
    let missing_existing = combine_object_reports(&[without.clone(), with_branches.clone()]);
    assert!(
        missing_existing["data"][0]["files"][0]
            .get("branches")
            .is_none()
    );
    let missing_incoming = combine_object_reports(&[with_branches, without]);
    assert_eq!(missing_incoming["data"][0]["files"][0]["branches"][0][4], 1);
}

#[test]
fn report_command_reuses_profiles_without_running_tests() {
    let arguments = report_arguments(std::path::Path::new("report.json"));
    assert_eq!(
        arguments.get(0..4),
        Some(
            [
                "+nightly".to_owned(),
                "llvm-cov".to_owned(),
                "report".to_owned(),
                "--json".to_owned()
            ]
            .as_slice()
        )
    );
    assert!(arguments.contains(&"--include-build-script".to_owned()));
    assert!(arguments.contains(&"/tests/fixtures/".to_owned()));
    assert!(!arguments.contains(&"test".to_owned()));
}

#[test]
fn report_only_rejects_missing_profile_data_before_invoking_cargo() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    fs::create_dir_all(fixture.path().join("target/llvm-cov-stale"))
        .expect("create empty coverage target");
    let error = report(fixture.path()).expect_err("missing profiles must fail");
    assert!(
        error.to_string().contains("no profraw/profdata"),
        "{error:#}"
    );
}

#[test]
fn assigns_each_gate_process_an_isolated_llvm_cov_target() {
    let root = tempfile::tempdir().expect("coverage fixture");
    let target = coverage_target_directory(root.path());
    assert_eq!(target.parent(), Some(root.path().join("target").as_path()));
    assert!(
        target
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("llvm-cov-"))
    );

    let command = coverage_command(root.path(), &target, &["--version".to_owned()]);
    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "CARGO_LLVM_COV_TARGET_DIR")
            .and_then(|(_, value)| value),
        Some(target.as_os_str())
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "LLVM_COV_FLAGS")
            .and_then(|(_, value)| value),
        Some(std::ffi::OsStr::new("--threads=1 --num-threads=1"))
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "LLVM_COV_NUM_THREADS")
            .and_then(|(_, value)| value),
        Some(std::ffi::OsStr::new("1"))
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "LLVM_PROFILE_FILE")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().contains("%m-%p")),
        Some(true)
    );
    for name in ["LLVM_COV", "LLVM_PROFDATA"] {
        let value = command
            .get_envs()
            .find(|(env, _)| *env == name)
            .and_then(|(_, value)| value)
            .expect("matching LLVM tool path");
        assert!(value.to_string_lossy().contains("llvm"));
    }
}

#[cfg(unix)]
#[test]
fn omits_llvm_tool_overrides_when_rustc_sysroot_lookup_fails() {
    const CHILD: &str = "CLAUDEX_COVERAGE_GATE_TOOL_LOOKUP_CHILD";
    if env::var_os(CHILD).is_some() {
        let fixture = tempfile::tempdir().expect("tool lookup fixture");
        let target = fixture.path().join("target/llvm-cov-tools");
        let command = coverage_command(fixture.path(), &target, &["--version".to_owned()]);
        assert!(command.get_envs().all(|(name, _)| {
            name != std::ffi::OsStr::new("LLVM_COV")
                && name != std::ffi::OsStr::new("LLVM_PROFDATA")
        }));
        return;
    }

    let fixture = tempfile::tempdir().expect("fake rustc fixture");
    let rustc = fixture.path().join("rustc");
    fs::write(&rustc, "#!/bin/sh\nexit 1\n").expect("fake rustc");
    fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755)).expect("fake rustc executable");
    let original_path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths(
        std::iter::once(fixture.path().to_path_buf())
            .chain(env::split_paths(&original_path).filter(|path| !path.as_os_str().is_empty())),
    )
    .expect("test PATH");
    let status = Command::new(env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "coverage_gate::tests::omits_llvm_tool_overrides_when_rustc_sysroot_lookup_fails",
        ])
        .env(CHILD, "1")
        .env("PATH", path)
        .status()
        .expect("run tool lookup child");
    assert!(status.success());
}

#[test]
fn retries_llvm_cov_json_export_on_segfault_or_corrupt_profraw() {
    use super::runner::should_retry_llvm_cov_export;

    let json = [
        "+nightly".to_owned(),
        "llvm-cov".to_owned(),
        "--json".to_owned(),
    ];
    let clean = [
        "+nightly".to_owned(),
        "llvm-cov".to_owned(),
        "clean".to_owned(),
    ];
    assert!(should_retry_llvm_cov_export(
        &json,
        std::process::ExitStatus::from_raw(libc::SIGSEGV)
    ));
    assert!(
        !should_retry_llvm_cov_export(&json, std::process::ExitStatus::from_raw(1 << 8)),
        "corrupt profraw merge (exit 1) must fail without dropping profiles"
    );
    assert!(!should_retry_llvm_cov_export(
        &clean,
        std::process::ExitStatus::from_raw(libc::SIGSEGV)
    ));
    assert!(
        !should_retry_llvm_cov_export(&json, std::process::ExitStatus::from_raw(101 << 8)),
        "failing tests (exit 101) must not look like a merge flake"
    );
}

#[test]
fn removes_only_successful_isolated_coverage_artifacts() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    let succeeded = fixture.path().join("succeeded");
    fs::create_dir(&succeeded).expect("create successful artifacts");
    discard_successful_artifacts(&succeeded, Ok(())).expect("remove successful artifacts");
    assert!(!succeeded.exists());

    let failed = fixture.path().join("failed");
    fs::create_dir(&failed).expect("create failed artifacts");
    assert!(
        discard_successful_artifacts(&failed, Err(anyhow::anyhow!("coverage failed"))).is_err()
    );
    assert!(failed.exists());
}

#[test]
fn discard_reports_when_successful_artifact_removal_fails() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    let not_a_directory = fixture.path().join("not-a-directory");
    fs::write(&not_a_directory, "leave me").expect("write non-directory artifact");
    let error = discard_successful_artifacts(&not_a_directory, Ok(()))
        .expect_err("removing a file path must surface the IO failure");
    assert!(error.to_string().contains("failed to remove"), "{error:#}");
    assert!(not_a_directory.is_file());
}

#[test]
fn prunes_stale_coverage_artifacts_but_keeps_current_and_live_runs() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    let target = fixture.path().join("target");
    fs::create_dir(&target).expect("create target directory");
    let stale = target.join("llvm-cov-stale");
    let current = target.join("llvm-cov-current");
    let live = target.join(format!("llvm-cov-{}", std::process::id()));
    for artifact in [&stale, &current, &live] {
        fs::create_dir(artifact).expect("create coverage artifact");
        fs::File::open(artifact)
            .expect("open coverage artifact")
            .set_times(fs::FileTimes::new().set_modified(
                std::time::SystemTime::now()
                    - crate::cache_hygiene::COVERAGE_TARGET_RETENTION
                    - std::time::Duration::from_secs(1),
            ))
            .expect("age coverage artifact");
    }
    let non_target = target.join("not-a-coverage-target");
    fs::create_dir(&non_target).expect("create non-target directory");
    fs::File::open(&non_target)
        .expect("open non-target directory")
        .set_times(fs::FileTimes::new().set_modified(
            std::time::SystemTime::now()
                - crate::cache_hygiene::COVERAGE_TARGET_RETENTION
                - std::time::Duration::from_secs(1),
        ))
        .expect("age non-target directory");

    prune_stale_coverage_artifacts(fixture.path(), &current, std::time::SystemTime::now())
        .expect("prune stale coverage artifacts");

    assert!(!stale.exists());
    assert!(current.exists());
    assert!(live.exists());
    assert!(non_target.exists());
}

#[test]
fn pruning_reports_a_non_directory_target_root() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    fs::write(fixture.path().join("target"), "not a directory").expect("write target file");
    let error = prune_stale_coverage_artifacts(
        fixture.path(),
        &fixture.path().join("target/llvm-cov-current"),
        std::time::SystemTime::now(),
    )
    .expect_err("target root is not a directory");
    assert!(error.to_string().contains("read"));
}

#[test]
fn pruning_a_missing_target_root_is_a_no_op() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    prune_stale_coverage_artifacts(
        fixture.path(),
        &fixture.path().join("target/llvm-cov-current"),
        std::time::SystemTime::now(),
    )
    .expect("missing target root must not fail the gate");
}

#[test]
fn pruning_ignores_non_directories_and_unrecognized_file_entries() {
    let fixture = tempfile::tempdir().expect("coverage fixture");
    let target_root = fixture.path().join("target");
    fs::create_dir(&target_root).expect("target directory");
    let coverage_named_file = target_root.join("llvm-cov-not-a-directory");
    let unrecognized_file = target_root.join("coverage-notes.txt");
    fs::write(&coverage_named_file, "not a target").expect("coverage-named file");
    fs::write(&unrecognized_file, "not a target").expect("unrecognized file");

    shrink_failed_coverage_target(&coverage_named_file).expect("non-directory shrink is a no-op");
    prune_stale_coverage_artifacts(
        fixture.path(),
        &target_root.join("llvm-cov-current"),
        std::time::SystemTime::now(),
    )
    .expect("file entries must not prevent pruning");

    assert!(coverage_named_file.is_file());
    assert!(unrecognized_file.is_file());
}

#[test]
fn prunes_excess_young_failed_coverage_targets_and_shrinks_kept_ones() {
    let fixture = tempfile::tempdir().expect("coverage keep fixture");
    let target = fixture.path().join("target");
    fs::create_dir(&target).expect("target");
    let current = target.join("llvm-cov-current");
    fs::create_dir(&current).expect("current");
    let mut failed = Vec::new();
    for name in ["llvm-cov-21", "llvm-cov-22", "llvm-cov-23"] {
        let path = target.join(name);
        fs::create_dir(&path).expect("failed target");
        fs::create_dir(path.join("debug")).expect("instrumented debug");
        fs::write(path.join("branch-coverage.json"), "{}").expect("report");
        failed.push(path);
    }
    fs::File::open(&failed[0])
        .expect("open oldest")
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(30)),
        )
        .expect("age oldest");
    prune_stale_coverage_artifacts(fixture.path(), &current, std::time::SystemTime::now())
        .expect("prune excess failed targets");
    assert!(!failed[0].exists());
    assert!(failed[1].join("branch-coverage.json").is_file());
    assert!(!failed[1].join("debug").exists());
    assert!(failed[2].join("branch-coverage.json").is_file());
    assert!(!failed[2].join("debug").exists());
}

#[test]
fn shrink_keeps_diagnosis_files_and_drops_instrumented_trees() {
    let fixture = tempfile::tempdir().expect("shrink fixture");
    let target = fixture.path().join("llvm-cov-9");
    fs::create_dir(&target).expect("target");
    fs::create_dir(target.join("debug")).expect("debug");
    fs::write(target.join("branch-coverage.json"), "{}").expect("json");
    fs::write(target.join("claudex.profdata"), "p").expect("profdata");
    fs::write(target.join("noise.bin"), "x").expect("noise");
    shrink_failed_coverage_target(&target).expect("shrink");
    assert!(target.join("branch-coverage.json").is_file());
    assert!(target.join("claudex.profdata").is_file());
    assert!(target.join("CACHEDIR.TAG").is_file());
    assert!(!target.join("debug").exists());
    assert!(!target.join("noise.bin").exists());
}

#[test]
fn production_entrypoint_preserves_failed_isolated_artifacts() {
    if env::var_os("CLAUDEX_COVERAGE_GATE_CHILD").is_some() {
        let root = tempfile::tempdir().expect("coverage fixture");
        let target = coverage_target_directory(root.path());
        let error = super::run(root.path()).expect_err("branch coverage failure");
        assert!(error.to_string().contains("branch coverage failed"));
        assert!(target.is_dir());
        return;
    }

    let fixture = tempfile::tempdir().expect("coverage fixture");
    let cargo = fixture.path().join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nmkdir -p \"$CARGO_LLVM_COV_TARGET_DIR\"\n[ \"$3\" = clean ] && exit 0\nexit 19\n",
    )
    .expect("fake cargo");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("executable cargo");
    let original_path = env::var_os("PATH").unwrap_or_default();
    let fixture_path = fixture.path().to_path_buf();
    let path = env::join_paths(
        std::iter::once(fixture_path)
            .chain(env::split_paths(&original_path).filter(|path| !path.as_os_str().is_empty())),
    )
    .expect("test PATH");
    let status = Command::new(env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "coverage_gate::tests::production_entrypoint_preserves_failed_isolated_artifacts",
        ])
        .env("CLAUDEX_COVERAGE_GATE_CHILD", "1")
        .env("PATH", path)
        .status()
        .expect("run isolated child test");
    assert!(status.success());
}

#[test]
fn accepts_a_passing_report_and_ignores_nonproduction_files() {
    let fixture = report_fixture(95.0, 95.0);
    audit_report(fixture.path(), &fixture.path().join("report.json")).expect("passing coverage");
    assert!(
        fixture
            .path()
            .join("target/coverage-last/metrics.json")
            .is_file()
    );
    assert!(
        fixture
            .path()
            .join("target/coverage-last/branch-coverage.json")
            .is_file()
    );
}

#[test]
fn rejects_a_report_below_the_committed_baseline() {
    let fixture = report_fixture(95.0, 100.0);
    fs::write(
        fixture.path().join("coverage-baseline.json"),
        br#"{"lines":95.0,"functions":95.0,"regions":95.0,"branches":96.0}"#,
    )
    .expect("baseline");
    let error = audit_report(fixture.path(), &fixture.path().join("report.json"))
        .expect_err("baseline drop");
    assert!(error.to_string().contains("below baseline"), "{error:#}");
}

#[test]
fn baseline_allow_file_permits_a_coverage_drop() {
    let fixture = report_fixture(95.0, 100.0);
    fs::write(
        fixture.path().join("coverage-baseline.json"),
        br#"{"lines":95.0,"functions":95.0,"regions":95.0,"branches":96.0}"#,
    )
    .expect("baseline");
    fs::write(fixture.path().join("coverage-baseline.allow"), "approved\n").expect("allow file");

    audit_report(fixture.path(), &fixture.path().join("report.json"))
        .expect("allow file permits baseline drop");
}

#[test]
fn baseline_allow_env_permits_a_coverage_drop() {
    const CHILD: &str = "CLAUDEX_COVERAGE_BASELINE_ALLOW_CHILD";
    if env::var_os(CHILD).is_some() {
        let fixture = report_fixture(95.0, 100.0);
        fs::write(
            fixture.path().join("coverage-baseline.json"),
            br#"{"lines":95.0,"functions":95.0,"regions":95.0,"branches":96.0}"#,
        )
        .expect("baseline");
        audit_report(fixture.path(), &fixture.path().join("report.json"))
            .expect("allow environment permits baseline drop");
        return;
    }

    let status = Command::new(env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "coverage_gate::tests::baseline_allow_env_permits_a_coverage_drop",
        ])
        .env(CHILD, "1")
        .env("CLAUDEX_COVERAGE_ALLOW_DROP", "1")
        .status()
        .expect("run isolated baseline-allow child");
    assert!(status.success());
}

#[test]
fn report_only_reuses_the_retained_successful_report() {
    let fixture = report_fixture(100.0, 100.0);
    let retained = fixture.path().join("target/coverage-last");
    fs::create_dir_all(&retained).expect("retained coverage directory");
    fs::copy(
        fixture.path().join("report.json"),
        retained.join("branch-coverage.json"),
    )
    .expect("retained report");
    super::report(fixture.path()).expect("retained report");
}

#[test]
fn runs_the_complete_gate_with_an_injected_command() {
    let fixture = report_fixture(100.0, 100.0);
    let target = fixture.path().join("target/llvm-cov-test");
    fs::create_dir_all(&target).expect("target directory");
    fs::copy(
        fixture.path().join("report.json"),
        target.join("branch-coverage.json"),
    )
    .expect("branch report");
    let mut calls = 0;
    run_with(
        fixture.path(),
        &target,
        |root, command_target, arguments| {
            calls += 1;
            assert_eq!(root, fixture.path());
            assert_eq!(command_target, target);
            assert!(!arguments.is_empty());
            Ok(std::process::ExitStatus::from_raw(0))
        },
    )
    .expect("coverage gate");
    assert_eq!(calls, 2);
}

#[test]
fn rejects_low_branches_and_low_production_lines() {
    let branches = report_fixture(94.9, 100.0);
    let error = audit_report(branches.path(), &branches.path().join("report.json"))
        .expect_err("low branches");
    assert!(error.to_string().contains("branches: 94.90%"));

    let lines = report_fixture(100.0, 94.9);
    let error =
        audit_report(lines.path(), &lines.path().join("report.json")).expect_err("low lines");
    assert!(error.to_string().contains("src/module.rs: 94.90%"));

    let functions = report_fixture(100.0, 100.0);
    let report = functions.path().join("report.json");
    let mut document: Value =
        serde_json::from_slice(&fs::read(&report).expect("read report")).expect("JSON");
    document["data"][0]["totals"]["functions"] = json!({"covered":949,"count":1000});
    fs::write(&report, serde_json::to_vec(&document).expect("JSON")).expect("write report");
    let error = audit_report(functions.path(), &report).expect_err("low functions");
    assert!(error.to_string().contains("functions: 94.90%"));
}

#[test]
fn merges_duplicate_branch_instances_by_source_location() {
    let root = tempfile::tempdir().expect("branch fixture");
    let source = root.path().join("src/module.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "pub fn covered() {}\n").unwrap();
    let data = json!({
        "totals":{"branches":{"covered":0,"count":4}},
        "files":[{
            "filename":source,
            "branches":[
                [1,1,1,4,3,0,0,0,4],
                [1,1,1,4,0,2,0,0,4]
            ],
            "summary":{"lines":{"covered":1,"count":1}}
        }]
    });
    assert_eq!(source_branch_percent(root.path(), &data).unwrap(), 100.0);
}

#[test]
fn rejects_malformed_branch_records_without_falling_back_to_totals() {
    let root = tempfile::tempdir().expect("branch fixture");
    let source = root.path().join("src/lib.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "pub fn covered() {}\n").unwrap();

    let object_record = json!({
        "files":[{"filename":source,"branches":[{"line":1}]}]
    });
    assert!(source_branch_percent(root.path(), &object_record).is_err());

    let incomplete_record = json!({
        "files":[{"filename":source,"branches":[[1, 1, 1]]}]
    });
    assert!(source_branch_percent(root.path(), &incomplete_record).is_err());
}

#[test]
fn rejects_malformed_reports() {
    let fixture = tempfile::tempdir().expect("fixture");
    let report = fixture.path().join("report.json");
    assert!(audit_report(fixture.path(), &report).is_err());
    fs::write(&report, b"not JSON").expect("write report");
    assert!(audit_report(fixture.path(), &report).is_err());
    fs::write(&report, b"{}").expect("write report");
    assert!(audit_report(fixture.path(), &report).is_err());
    fs::write(
        &report,
        br#"{"data":[{"totals":{"branches":{"covered":95,"count":100}}}]}"#,
    )
    .expect("write report");
    assert!(audit_report(fixture.path(), &report).is_err());
}

#[test]
fn executes_clean_before_coverage_and_reports_command_failures() {
    let fixture = tempfile::tempdir().expect("fixture");
    let report = fixture.path().join("report.json");
    let mut calls = Vec::new();
    run_commands(&report, |arguments| {
        calls.push(arguments.to_vec());
        Ok(std::process::ExitStatus::from_raw(0))
    })
    .expect("commands succeed");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][2], "clean");
    assert!(calls[1].contains(&"--branch".to_owned()));

    let error = run_commands(&report, |_| Ok(std::process::ExitStatus::from_raw(1 << 8)))
        .expect_err("clean failure");
    assert!(error.to_string().contains("coverage clean failed"));

    let mut call = 0;
    let error = run_commands(&report, |_| {
        call += 1;
        Ok(std::process::ExitStatus::from_raw(
            usize::from(call == 2) as i32 * (1 << 8),
        ))
    })
    .expect_err("coverage failure");
    assert!(error.to_string().contains("branch coverage failed"));

    assert!(run_commands(&report, |_| anyhow::bail!("spawn clean")).is_err());
    let mut call = 0;
    assert!(
        run_commands(&report, |_| {
            call += 1;
            if call == 2 {
                anyhow::bail!("spawn coverage");
            }
            Ok(std::process::ExitStatus::from_raw(0))
        })
        .is_err()
    );
    assert!(
        command_status(
            fixture.path(),
            &fixture.path().join("target/llvm-cov-test"),
            &["--version".to_owned()],
        )
        .expect("cargo version")
        .success()
    );
}

#[test]
fn command_status_rejects_corrupt_profile_merge_without_dropping_profiles() {
    if env::var_os("CLAUDEX_COVERAGE_GATE_RETRY_CHILD").is_some() {
        let root = tempfile::tempdir().expect("retry fixture");
        let target = root.path().join("target/llvm-cov-retry");
        fs::create_dir_all(&target).expect("retry target");
        let status = command_status(
            root.path(),
            &target,
            &[
                "+nightly".to_owned(),
                "llvm-cov".to_owned(),
                "--json".to_owned(),
            ],
        )
        .expect("retryable json export");
        assert!(!status.success(), "corrupt merge must fail the gate");
        assert!(
            target.join("bad.profraw").is_file(),
            "corrupt profiles must remain available for diagnosis"
        );
        return;
    }

    let fixture = tempfile::tempdir().expect("retry PATH fixture");
    let cargo = fixture.path().join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nmkdir -p \"$CARGO_LLVM_COV_TARGET_DIR\"\ntouch \"$CARGO_LLVM_COV_TARGET_DIR/bad.profraw\"\nexit 1\n",
    )
    .expect("fake cargo");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("executable cargo");
    let original_path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths(
        std::iter::once(fixture.path().to_path_buf())
            .chain(env::split_paths(&original_path).filter(|path| !path.as_os_str().is_empty())),
    )
    .expect("test PATH");
    let status = Command::new(env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "coverage_gate::tests::command_status_rejects_corrupt_profile_merge_without_dropping_profiles",
        ])
        .env("CLAUDEX_COVERAGE_GATE_RETRY_CHILD", "1")
        .env("PATH", path)
        .status()
        .expect("run retry child");
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn command_status_retries_json_export_after_sigsegv() {
    const CHILD: &str = "CLAUDEX_COVERAGE_GATE_SIGSEGV_CHILD";
    if env::var_os(CHILD).is_some() {
        let root = tempfile::tempdir().expect("retry fixture");
        let target = root.path().join("target/llvm-cov-retry");
        fs::create_dir_all(&target).expect("retry target");
        let status = command_status(
            root.path(),
            &target,
            &[
                "+nightly".to_owned(),
                "llvm-cov".to_owned(),
                "--json".to_owned(),
            ],
        )
        .expect("retryable json export");
        assert!(status.success(), "SIGSEGV export should succeed on retry");
        assert!(
            target.join("sigsegv-seen").is_file(),
            "fake cargo should have executed the failing first attempt"
        );
        assert!(
            target.join("retry-seen").is_file(),
            "fake cargo should have executed exactly one retry"
        );
        return;
    }

    let fixture = tempfile::tempdir().expect("retry PATH fixture");
    let cargo = fixture.path().join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nif [ ! -f \"$CARGO_LLVM_COV_TARGET_DIR/sigsegv-seen\" ]; then\n  : > \"$CARGO_LLVM_COV_TARGET_DIR/sigsegv-seen\"\n  kill -SEGV $$\nfi\nif [ -f \"$CARGO_LLVM_COV_TARGET_DIR/retry-seen\" ]; then\n  exit 17\nfi\n: > \"$CARGO_LLVM_COV_TARGET_DIR/retry-seen\"\nexit 0\n",
    )
    .expect("fake cargo");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("fake cargo executable");
    let original_path = env::var_os("PATH").unwrap_or_default();
    let path = env::join_paths(
        std::iter::once(fixture.path().to_path_buf())
            .chain(env::split_paths(&original_path).filter(|path| !path.as_os_str().is_empty())),
    )
    .expect("test PATH");
    let status = Command::new(env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "coverage_gate::tests::command_status_retries_json_export_after_sigsegv",
        ])
        .env(CHILD, "1")
        .env("PATH", path)
        .status()
        .expect("run SIGSEGV retry child");
    assert!(status.success());
}

#[test]
fn detects_missing_and_unexpected_production_files() {
    let missing = report_fixture(100.0, 100.0);
    let report_path = missing.path().join("report.json");
    let mut report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).expect("read")).expect("JSON");
    let files = report["data"][0]["files"].as_array_mut().expect("files");
    files.retain(|file| {
        !file["filename"]
            .as_str()
            .expect("filename")
            .ends_with("src/module.rs")
    });
    fs::write(&report_path, serde_json::to_vec(&report).expect("JSON")).expect("write");
    assert!(
        audit_report(missing.path(), &report_path)
            .expect_err("missing file")
            .to_string()
            .contains("missing from report")
    );

    let unexpected = report_fixture(100.0, 100.0);
    let report_path = unexpected.path().join("report.json");
    let mut report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).expect("read")).expect("JSON");
    report["data"][0]["files"]
        .as_array_mut()
        .expect("files")
        .push(json!({
            "filename":format!("{}/src/extra.rs", unexpected.path().display()),
            "summary":{"lines":{"covered":1,"count":1}}
        }));
    fs::write(&report_path, serde_json::to_vec(&report).expect("JSON")).expect("write");
    assert!(
        audit_report(unexpected.path(), &report_path)
            .expect_err("unexpected file")
            .to_string()
            .contains("unexpected production file")
    );
}

#[test]
fn handles_zero_counts_and_test_source_names() {
    assert_eq!(
        coverage_percent(&json!({"coverage":{"covered":0,"count":0}}), "/coverage")
            .expect("zero count"),
        100.0
    );
    assert!(coverage_percent(&json!({"coverage":{"count":1}}), "/coverage").is_err());
    assert!(coverage_percent(&json!({"coverage":{"covered":1}}), "/coverage").is_err());
    assert!(is_test_only_source(std::path::Path::new(
        "src/module_tests.rs"
    )));
    assert!(is_test_only_source(std::path::Path::new(
        "src/stream/tests.rs"
    )));
    assert!(is_test_only_source(std::path::Path::new(
        "src/grok_acp/test_support.rs"
    )));
    assert!(!is_test_only_source(std::path::Path::new("src/module.rs")));
    assert!(!is_test_only_source(std::path::Path::new(
        "src/non-utf8-placeholder"
    )));
    assert_non_executable_sources();
}

fn assert_non_executable_sources() {
    for path in [
        "src/anthropic.rs",
        "src/anthropic/bridge_instructions.rs",
        "src/anthropic/bridge_types.rs",
        "src/anthropic/subscription_request.rs",
        "src/anthropic/stream/turn.rs",
        "src/provider_config/types.rs",
    ] {
        assert!(
            is_non_executable_source(std::path::Path::new(path)),
            "{path}"
        );
    }
    assert!(!is_non_executable_source(std::path::Path::new(
        "src/module.rs"
    )));
    assert!(!is_non_executable_source(std::path::Path::new(
        "src/web_search/parse.rs"
    )));
    for executable in [
        "src/lib.rs",
        "src/command_code_acp/agent_acp.rs",
        "src/command_code_acp/mod.rs",
        "src/grok_acp/test_support.rs",
    ] {
        assert!(!is_non_executable_source(std::path::Path::new(executable)));
    }
}

#[test]
fn production_coverage_exceptions_match_the_exact_manifest() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let production_files = production_source_files(root);
    assert!(!production_files.is_empty());
    assert_inventory_annotations(root, &production_files);
    assert_exception_manifest(root, &production_files);
}

#[test]
fn production_inventory_includes_build_script_and_rejects_its_hidden_annotation() {
    let fixture = tempfile::tempdir().expect("production inventory fixture");
    fs::create_dir(fixture.path().join("src")).expect("source directory");
    fs::write(fixture.path().join("src/lib.rs"), "pub fn measured() {}\n").expect("source fixture");
    fs::write(
        fixture.path().join("build.rs"),
        format!("{COVERAGE_OFF_ATTRIBUTE}\nfn main() {{}}\n"),
    )
    .expect("build script fixture");

    let production_files = production_source_files(fixture.path());
    assert!(production_files.contains(&std::path::PathBuf::from("build.rs")));
    let result = std::panic::catch_unwind(|| {
        assert_inventory_annotations(fixture.path(), &production_files);
    });
    assert!(
        result.is_err(),
        "unmanifested build.rs annotation must fail"
    );
}

const COVERAGE_OFF_ATTRIBUTE: &str = "#[cfg_attr(coverage_nightly, coverage(off))]";

fn exception_counts() -> std::collections::BTreeMap<&'static str, usize> {
    INSTRUMENTATION_EXCEPTIONS.iter().fold(
        std::collections::BTreeMap::<&str, usize>::new(),
        |mut counts, exception| {
            *counts.entry(exception.path).or_default() += 1;
            counts
        },
    )
}

fn assert_inventory_annotations(root: &std::path::Path, production_files: &[std::path::PathBuf]) {
    let expected_counts = exception_counts();
    let mut actual_total = 0;
    for relative in production_files {
        let source = fs::read_to_string(root.join(relative)).expect("read production source");
        let actual = production_coverage_off_lines(&source, COVERAGE_OFF_ATTRIBUTE);
        actual_total += actual.len();
        assert_eq!(
            actual.len(),
            expected_counts
                .get(relative.to_str().expect("UTF-8 production path"))
                .copied()
                .unwrap_or_default(),
            "{} has unmanifested or missing production coverage(off) at lines {actual:?}",
            relative.display()
        );
    }
    assert_eq!(
        actual_total,
        INSTRUMENTATION_EXCEPTIONS.len(),
        "production coverage(off) count must equal the exact manifest"
    );
}

fn assert_exception_manifest(root: &std::path::Path, production_files: &[std::path::PathBuf]) {
    assert_eq!(INSTRUMENTATION_EXCEPTIONS.len(), 4);
    assert_eq!(
        INSTRUMENTATION_EXCEPTIONS
            .iter()
            .map(|exception| (exception.path, exception.symbol))
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        INSTRUMENTATION_EXCEPTIONS.len(),
        "manifest path+symbol entries must be unique"
    );
    for path in exception_counts().keys() {
        assert!(
            production_files
                .iter()
                .any(|candidate| candidate == std::path::Path::new(path)),
            "manifest path is not in the production inventory: {path}"
        );
    }

    let mut rust_files = Vec::new();
    crate::build_support::collect_rust_files(&root.join("src"), &mut rust_files);
    for exception in INSTRUMENTATION_EXCEPTIONS {
        assert_exception_entry(root, &rust_files, exception);
    }
}

fn assert_exception_entry(
    root: &std::path::Path,
    rust_files: &[std::path::PathBuf],
    exception: &super::report::InstrumentationException,
) {
    assert!(matches!(
        exception.reason_category,
        "async-trait-codegen" | "pre-exec-syscall"
    ));
    let source = fs::read_to_string(root.join(exception.path)).expect("read exception source");
    let marker = format!(
        "// coverage-exception: {}; symbol={}; evidence={}",
        exception.reason_category, exception.symbol, exception.test_evidence
    );
    let marker_start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("{} is missing marker `{marker}`", exception.path));
    let after_marker = &source[marker_start + marker.len()..];
    let attribute_start = after_marker
        .find(COVERAGE_OFF_ATTRIBUTE)
        .unwrap_or_else(|| panic!("{} marker is not followed by coverage(off)", exception.path));
    assert!(
        after_marker[attribute_start + COVERAGE_OFF_ATTRIBUTE.len()..].contains(exception.symbol)
    );
    let test_name = exception.test_evidence.rsplit("::").next().unwrap();
    assert!(rust_files.iter().any(|path| {
        fs::read_to_string(path).is_ok_and(|contents| contents.contains(&format!("fn {test_name}")))
    }));
}

fn production_source_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    crate::build_support::collect_rust_files(&root.join("src"), &mut files);
    let mut production = files
        .into_iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(std::path::Path::to_owned))
        .filter(|path| !is_test_only_source(path))
        .collect::<Vec<_>>();
    if root.join("build.rs").is_file() {
        production.push(std::path::PathBuf::from("build.rs"));
    }
    production
}

fn production_coverage_off_lines(source: &str, attribute: &str) -> Vec<usize> {
    let lines = source.lines().collect::<Vec<_>>();
    lines
        .iter()
        .enumerate()
        .filter(|(index, line)| {
            if line.trim() != attribute {
                return false;
            }
            !lines[..*index]
                .iter()
                .rev()
                .map(|line| line.trim())
                .take_while(|line| {
                    line.is_empty() || line.starts_with("//") || line.starts_with("#[")
                })
                .any(|line| line == "#[cfg(test)]")
        })
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn source_lines_use_countable_segments_instead_of_async_summary_artifacts() {
    let mapped = json!({
        "segments":[
            [10, 1, 4, true, true, false],
            [10, 8, 0, false, false, false],
            [11, 1, 0, true, true, false]
        ]
    });
    assert_eq!(source_line_percent(&mapped).unwrap(), Some(50.0));
    assert_eq!(
        source_line_percent(&json!({"segments":[[20, 1, 0, false, false, false]]})).unwrap(),
        None
    );
    assert_eq!(source_line_percent(&json!({})).unwrap(), None);
    assert!(source_line_percent(&json!({"segments":[[1]]})).is_err());
}

fn report_fixture(branches: f64, lines: f64) -> tempfile::TempDir {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().display().to_string();
    fs::create_dir_all(fixture.path().join("src/anthropic")).expect("source directory");
    for file in [
        "src/lib.rs",
        "src/module.rs",
        "src/anthropic/protocol_tests.rs",
        "build.rs",
    ] {
        fs::write(fixture.path().join(file), "").expect("source file");
    }
    let branch_covered = (branches * 10.0).round() as u64;
    let line_covered = (lines * 10.0).round() as u64;
    let report = json!({
        "data":[{
            "totals":{
                "branches":{"covered":branch_covered,"count":1000},
                "functions":{"covered":1000,"count":1000},
                "regions":{"covered":1000,"count":1000},
                "lines":{"covered":1000,"count":1000}
            },
            "files":[
                {
                    "filename":format!("{root}/src/lib.rs"),
                    "summary":{"lines":{"covered":line_covered,"count":1000}}
                },
                {
                    "filename":format!("{root}/src/module.rs"),
                    "summary":{"lines":{"covered":line_covered,"count":1000}}
                },
                {
                    "filename":format!("{root}/build.rs"),
                    "summary":{"lines":{"covered":1,"count":1}}
                },
                {
                    "filename":format!("{root}/src/anthropic/protocol_tests.rs"),
                    "summary":{"lines":{"covered":0,"count":10}}
                },
                {
                    "filename":format!("{root}/tests/example.rs"),
                    "summary":{"lines":{"covered":0,"count":10}}
                }
            ]
        }]
    });
    fs::write(
        fixture.path().join("report.json"),
        serde_json::to_vec(&report).expect("serialize"),
    )
    .expect("write report");
    fixture
}
