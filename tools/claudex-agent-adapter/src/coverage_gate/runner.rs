use std::{
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};

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
    if retained.is_file() {
        return super::audit_report(root, &retained);
    }
    let target = existing_coverage_target(root)?;
    let report = target.join("branch-coverage.json");
    let status = command_status(root, &target, &report_arguments(&report))?;
    require_success(status, "coverage report")?;
    super::audit_report(root, &report)
}

pub(super) fn coverage_target_directory(root: &Path) -> PathBuf {
    root.join("target")
        .join(format!("{COVERAGE_TARGET_PREFIX}{}", std::process::id()))
}

fn existing_coverage_target(root: &Path) -> Result<PathBuf> {
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

fn has_profile_data(root: &Path) -> bool {
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

fn matching_llvm_tool(name: &str) -> Option<PathBuf> {
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
