use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    process::Command,
};

use serde_json::{Value, json};

use super::runner::{
    COVERAGE_ARTIFACT_RETENTION, command_status, coverage_arguments, coverage_command,
    coverage_target_directory, discard_successful_artifacts, prune_stale_coverage_artifacts,
    run_commands, run_with,
};
use super::{
    audit_report, coverage_percent, is_non_executable_source, is_test_only_source,
    source_branch_percent, source_line_percent,
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
        should_retry_llvm_cov_export(&json, std::process::ExitStatus::from_raw(1 << 8)),
        "corrupt profraw merge (exit 1) must retry the json export"
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
                    - COVERAGE_ARTIFACT_RETENTION
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
                - COVERAGE_ARTIFACT_RETENTION
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
    assert!(!is_test_only_source(std::path::Path::new("src/module.rs")));
    assert!(!is_test_only_source(std::path::Path::new(
        "src/non-utf8-placeholder"
    )));
    assert!(is_non_executable_source(std::path::Path::new("src/lib.rs")));
    assert!(is_non_executable_source(std::path::Path::new(
        "src/anthropic/stream/turn.rs"
    )));
    assert!(is_non_executable_source(std::path::Path::new(
        "src/provider_config/types.rs"
    )));
    assert!(!is_non_executable_source(std::path::Path::new(
        "src/module.rs"
    )));
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
