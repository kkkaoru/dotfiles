use std::{
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{combine_object_reports, report::production_file};

const COVERAGE_TARGET_PREFIX: &str = "llvm-cov-";
const COVERAGE_TOOLCHAIN: &str = "+nightly";
const LLVM_COV_REPORT_THREADS: &str = "--threads=1 --num-threads=1";
// Failed reports remain available briefly for diagnosis, then a later coverage
// run reclaims their multi-gigabyte instrumented target directories.
pub(super) const COVERAGE_ARTIFACT_RETENTION: Duration = Duration::from_secs(10 * 60);

#[path = "runner_prune.rs"]
mod prune;
pub(super) use prune::prune_stale_coverage_artifacts;

pub fn run(root: &Path) -> Result<()> {
    let target = coverage_target_directory(root);
    prune_stale_coverage_artifacts(root, &target, SystemTime::now())?;
    discard_successful_artifacts(&target, run_with(root, &target, command_status))
}

pub fn report(root: &Path) -> Result<()> {
    let retained = root.join("target/coverage-last/branch-coverage.json");
    let target = match existing_coverage_target(root) {
        Ok(target) => target,
        Err(_error) if retained.is_file() => return super::audit_report(root, &retained),
        Err(error) => return Err(error),
    };
    let report = target.join("branch-coverage.json");
    let mut document = per_object_report(&target)?;
    recompute_production_totals(root, &mut document);
    std::fs::write(&report, serde_json::to_vec(&document)?)?;
    std::fs::copy(&report, &retained)
        .with_context(|| format!("failed to retain {}", report.display()))?;
    super::audit_report(root, &report)
}

pub(super) fn recompute_production_totals(root: &Path, document: &mut Value) {
    let Some(files) = document.pointer("/data/0/files").and_then(Value::as_array) else {
        return;
    };
    let mut totals = serde_json::Map::new();
    for metric in ["lines", "functions", "regions", "branches"] {
        let (covered, count) = files
            .iter()
            .filter_map(|file| production_file(root, file).map(|(_, value)| value))
            .filter_map(|file| file.pointer(&format!("/summary/{metric}")))
            .fold((0_u64, 0_u64), |(covered, count), summary| {
                (
                    covered + summary["covered"].as_u64().unwrap_or(0),
                    count + summary["count"].as_u64().unwrap_or(0),
                )
            });
        totals.insert(
            metric.to_owned(),
            serde_json::json!({"covered": covered, "count": count}),
        );
    }
    document["data"][0]["totals"] = Value::Object(totals);
}

pub(super) fn per_object_report(target: &Path) -> Result<Value> {
    let deps = target.join("debug/deps");
    let mut reports = Vec::new();
    for entry in std::fs::read_dir(deps).context("read coverage test binaries")? {
        let path = entry?.path();
        if !path.is_file() || path.extension().is_some() {
            continue;
        }
        let output = Command::new(matching_llvm_tool("llvm-cov").context("llvm-cov")?)
            .args(["export", "-format=text", "-instr-profile"])
            .arg(find_profile(target)?)
            .arg(&path)
            .output()
            .with_context(|| format!("export {}", path.display()))?;
        if output.status.success() {
            if let Ok(document) = serde_json::from_slice::<Value>(&output.stdout) {
                reports.push(document);
            }
        }
    }
    if reports.is_empty() {
        bail!("no per-object llvm-cov exports succeeded")
    }
    Ok(combine_object_reports(&reports))
}

pub(super) fn find_profile(target: &Path) -> Result<PathBuf> {
    std::fs::read_dir(target)?
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .find(|path| path.extension().is_some_and(|ext| ext == "profdata"))
        .context("no merged profdata found")
}

pub(super) fn coverage_target_directory(root: &Path) -> PathBuf {
    root.join("target")
        .join(format!("{COVERAGE_TARGET_PREFIX}{}", std::process::id()))
}

pub(super) fn existing_coverage_target(root: &Path) -> Result<PathBuf> {
    let target_root = root.join("target");
    let mut candidates = std::fs::read_dir(&target_root)
        .with_context(|| format!("failed to read {}", target_root.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let is_target = entry
                .file_name()
                .to_string_lossy()
                .starts_with(COVERAGE_TARGET_PREFIX);
            (is_target && path.is_dir()).then_some(path)
        })
        .filter(|path| has_profile_data(path))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    candidates
        .pop()
        .with_context(|| {
            format!(
                "no profraw/profdata found under {}; run `cargo coverage` to regenerate coverage artifacts",
                target_root.display()
            )
        })
}

pub(super) fn has_profile_data(root: &Path) -> bool {
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(path) else {
            continue;
        };
        if profile_entries(entries, &mut pending) {
            return true;
        }
    }
    false
}

fn profile_entries(entries: std::fs::ReadDir, pending: &mut Vec<PathBuf>) -> bool {
    for entry in entries.flatten() {
        if profile_entry(entry.path(), pending) {
            return true;
        }
    }
    false
}

fn profile_entry(path: PathBuf, pending: &mut Vec<PathBuf>) -> bool {
    if path.is_dir() {
        pending.push(path);
        return false;
    }
    path.extension()
        .is_some_and(|extension| extension == "profraw" || extension == "profdata")
}

/// Bound disk use from retained failed coverage reports without removing an
/// active sibling coverage run. Successful runs delete their own target below.
pub(super) fn run_with(
    root: &Path,
    target: &Path,
    mut execute: impl FnMut(&Path, &Path, &[String]) -> Result<ExitStatus>,
) -> Result<()> {
    let report = target.join("branch-coverage.json");
    run_commands(&report, |arguments| execute(root, target, arguments))?;
    super::audit_report(root, &report)
}

pub(super) fn discard_successful_artifacts(target: &Path, outcome: Result<()>) -> Result<()> {
    match outcome {
        Ok(()) => std::fs::remove_dir_all(target)
            .with_context(|| format!("failed to remove {}", target.display())),
        // Keep failed reports and instrumented artifacts available for coverage diagnosis.
        Err(error) => Err(error),
    }
}

pub(super) fn command_status(
    root: &Path,
    target: &Path,
    arguments: &[String],
) -> Result<ExitStatus> {
    let mut status = coverage_command(root, target, arguments)
        .status()
        .context("failed to run cargo")?;
    if should_retry_llvm_cov_export(arguments, status) {
        status = coverage_command(root, target, arguments)
            .status()
            .context("failed to retry cargo llvm-cov export")?;
    }
    Ok(status)
}

pub(super) fn coverage_command(root: &Path, target: &Path, arguments: &[String]) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(arguments)
        .current_dir(root)
        .env("CARGO_LLVM_COV_TARGET_DIR", target)
        // llvm-cov --branch export can SIGSEGV in
        // CoverageMapping::getInstantiationGroups when it fans out. Keep the
        // report single-threaded so the gate does not flake on Apple Silicon.
        .env("LLVM_COV_FLAGS", LLVM_COV_REPORT_THREADS)
        .env("LLVM_COV_NUM_THREADS", "1")
        .env("LLVM_PROFILE_FILE", target.join("claudex-%m-%p.profraw"));
    if let Some(tool) = matching_llvm_tool("llvm-cov") {
        command.env("LLVM_COV", tool);
    }
    if let Some(tool) = matching_llvm_tool("llvm-profdata") {
        command.env("LLVM_PROFDATA", tool);
    }
    command
}

pub(super) fn matching_llvm_tool(name: &str) -> Option<PathBuf> {
    let sysroot = Command::new("rustc")
        .args([COVERAGE_TOOLCHAIN, "--print", "sysroot"])
        .output()
        .ok()?;
    if !sysroot.status.success() {
        return None;
    }
    let target = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
    let target = target.replace("macos", "apple-darwin");
    let path = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim())
        .join("lib")
        .join("rustlib")
        .join(target)
        .join("bin")
        .join(name);
    path.is_file().then_some(path)
}

pub(super) fn should_retry_llvm_cov_export(arguments: &[String], status: ExitStatus) -> bool {
    if !arguments.iter().any(|argument| argument == "--json") {
        return false;
    }
    // SIGSEGV is the known Apple Silicon export flake. A status 1 is a
    // corrupt-profile merge and must fail rather than publish lower coverage.
    status.signal() == Some(libc::SIGSEGV)
}

pub(super) fn run_commands(
    report: &Path,
    mut execute: impl FnMut(&[String]) -> Result<ExitStatus>,
) -> Result<()> {
    let clean = [COVERAGE_TOOLCHAIN, "llvm-cov", "clean", "--workspace"].map(str::to_owned);
    require_success(
        execute(&clean).context("failed to clean previous coverage data")?,
        "coverage clean",
    )?;
    require_success(
        execute(&coverage_arguments(report)).context("failed to run branch coverage")?,
        "branch coverage",
    )
}

pub(super) fn coverage_arguments(report: &Path) -> Vec<String> {
    [
        COVERAGE_TOOLCHAIN,
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

#[cfg(test)]
pub(super) fn report_arguments(report: &Path) -> Vec<String> {
    [
        COVERAGE_TOOLCHAIN,
        "llvm-cov",
        "report",
        "--json",
        "--include-build-script",
        "--ignore-filename-regex",
        "/tests/fixtures/",
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
