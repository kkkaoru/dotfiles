use std::{
    path::Path,
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

mod report;
#[cfg(test)]
use report::is_test_only_source;
use report::{coverage_percent, production_line_failures, source_branch_percent};

const MINIMUM_PERCENT: f64 = 95.0;
const TOTAL_METRICS: [&str; 3] = ["lines", "functions", "regions"];

pub fn run(root: &Path) -> Result<()> {
    run_with(root, command_status)
}

fn run_with(
    root: &Path,
    mut execute: impl FnMut(&Path, &[String]) -> Result<ExitStatus>,
) -> Result<()> {
    let report = root.join("target/branch-coverage.json");
    run_commands(&report, |arguments| execute(root, arguments))?;
    audit_report(root, &report)
}

fn command_status(root: &Path, arguments: &[String]) -> Result<ExitStatus> {
    Command::new("cargo")
        .args(arguments)
        .current_dir(root)
        .status()
        .context("failed to run cargo")
}

fn run_commands(
    report: &Path,
    mut execute: impl FnMut(&[String]) -> Result<ExitStatus>,
) -> Result<()> {
    let clean = ["+nightly", "llvm-cov", "clean", "--workspace"].map(str::to_owned);
    let status = execute(&clean).context("failed to clean previous coverage data")?;
    require_success(status, "coverage clean")?;
    let status = execute(&coverage_arguments(report)).context("failed to run branch coverage")?;
    require_success(status, "branch coverage")
}

fn coverage_arguments(report: &Path) -> Vec<String> {
    [
        "+nightly",
        "llvm-cov",
        "--branch",
        "--all-targets",
        "--include-build-script",
        "--ignore-filename-regex",
        "/tests/fixtures/",
        "--json",
        "--output-path",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([report.display().to_string()])
    .collect()
}

fn require_success(status: ExitStatus, operation: &str) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    bail!("{operation} failed with {status}")
}

pub fn audit_report(root: &Path, report: &Path) -> Result<()> {
    let document: Value = serde_json::from_slice(
        &std::fs::read(report).with_context(|| format!("failed to read {}", report.display()))?,
    )
    .context("invalid llvm-cov JSON")?;
    let data = document
        .pointer("/data/0")
        .context("llvm-cov report has no data")?;
    let total_failures = TOTAL_METRICS
        .iter()
        .map(|metric| {
            let coverage = coverage_percent(data, &format!("/totals/{metric}"))?;
            Ok((coverage < MINIMUM_PERCENT).then(|| format!("{metric}: {coverage:.2}%")))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .chain({
            let coverage = source_branch_percent(root, data)?;
            (coverage < MINIMUM_PERCENT).then(|| format!("branches: {coverage:.2}%"))
        })
        .collect::<Vec<_>>();
    if !total_failures.is_empty() {
        bail!(
            "total coverage below {MINIMUM_PERCENT:.0}%:\n{}",
            total_failures.join("\n")
        );
    }
    let failures = production_line_failures(root, data)?;
    if failures.is_empty() {
        return Ok(());
    }
    bail!(
        "production files below {MINIMUM_PERCENT:.0}% line coverage:\n{}",
        failures.join("\n")
    )
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;

    use serde_json::{Value, json};

    use std::os::unix::process::ExitStatusExt;

    use super::{
        audit_report, command_status, coverage_arguments, coverage_percent, is_test_only_source,
        run_commands, run_with, source_branch_percent,
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
    fn accepts_a_passing_report_and_ignores_nonproduction_files() {
        let fixture = report_fixture(95.0, 95.0);
        audit_report(fixture.path(), &fixture.path().join("report.json"))
            .expect("passing coverage");
    }

    #[test]
    fn runs_the_complete_gate_with_an_injected_command() {
        let fixture = report_fixture(100.0, 100.0);
        fs::create_dir(fixture.path().join("target")).expect("target directory");
        fs::copy(
            fixture.path().join("report.json"),
            fixture.path().join("target/branch-coverage.json"),
        )
        .expect("branch report");
        let mut calls = 0;
        run_with(fixture.path(), |root, arguments| {
            calls += 1;
            assert_eq!(root, fixture.path());
            assert!(!arguments.is_empty());
            Ok(std::process::ExitStatus::from_raw(0))
        })
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
        assert!(error.to_string().contains("src/lib.rs: 94.90%"));

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
        let source = root.path().join("src/lib.rs");
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
            command_status(fixture.path(), &["--version".to_owned()])
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
                .ends_with("src/lib.rs")
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
    }

    fn report_fixture(branches: f64, lines: f64) -> tempfile::TempDir {
        let fixture = tempfile::tempdir().expect("fixture");
        let root = fixture.path().display().to_string();
        fs::create_dir_all(fixture.path().join("src/anthropic")).expect("source directory");
        for file in ["src/lib.rs", "src/anthropic/protocol_tests.rs", "build.rs"] {
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
}
