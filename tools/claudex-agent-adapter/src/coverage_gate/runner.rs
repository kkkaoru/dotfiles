use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};

const COVERAGE_TARGET_PREFIX: &str = "llvm-cov-";
// Failed reports remain available briefly for diagnosis, then a later coverage
// run reclaims their multi-gigabyte instrumented target directories.
pub(super) const COVERAGE_ARTIFACT_RETENTION: Duration = Duration::from_secs(10 * 60);

pub fn run(root: &Path) -> Result<()> {
    let target = coverage_target_directory(root);
    prune_stale_coverage_artifacts(root, &target, SystemTime::now())?;
    discard_successful_artifacts(&target, run_with(root, &target, command_status))
}

pub(super) fn coverage_target_directory(root: &Path) -> PathBuf {
    root.join("target")
        .join(format!("{COVERAGE_TARGET_PREFIX}{}", std::process::id()))
}

/// Bound disk use from retained failed coverage reports without removing an
/// active sibling coverage run. Successful runs delete their own target below.
pub(super) fn prune_stale_coverage_artifacts(
    root: &Path,
    current: &Path,
    now: SystemTime,
) -> Result<()> {
    let target_root = root.join("target");
    let entries = match fs::read_dir(&target_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("read {}", target_root.display())),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if stale_coverage_artifact(&entry, current, now)? {
            fs::remove_dir_all(&path)
                .with_context(|| format!("remove stale coverage artifact {}", path.display()))?;
        }
    }
    Ok(())
}

fn stale_coverage_artifact(entry: &fs::DirEntry, current: &Path, now: SystemTime) -> Result<bool> {
    let path = entry.path();
    if path == current || !is_coverage_target(entry) || live_coverage_process(entry) {
        return Ok(false);
    }
    let modified = entry
        .metadata()
        .with_context(|| format!("inspect coverage artifact {}", path.display()))?
        .modified()
        .with_context(|| format!("read modification time for {}", path.display()))?;
    Ok(now
        .duration_since(modified)
        .is_ok_and(|age| age >= COVERAGE_ARTIFACT_RETENTION))
}

fn is_coverage_target(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(COVERAGE_TARGET_PREFIX))
}

fn live_coverage_process(entry: &fs::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .and_then(|name| name.strip_prefix(COVERAGE_TARGET_PREFIX))
        .and_then(|process| process.parse::<i32>().ok())
        .is_some_and(process_is_alive)
}

fn process_is_alive(process: i32) -> bool {
    // SAFETY: signal zero performs no action; it only asks the OS whether the
    // process exists, which prevents pruning an active sibling coverage run.
    unsafe { libc::kill(process, 0) == 0 }
}

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
    coverage_command(root, target, arguments)
        .status()
        .context("failed to run cargo")
}

pub(super) fn coverage_command(root: &Path, target: &Path, arguments: &[String]) -> Command {
    let mut command = Command::new("cargo");
    command
        .args(arguments)
        .current_dir(root)
        .env("CARGO_LLVM_COV_TARGET_DIR", target);
    command
}

pub(super) fn run_commands(
    report: &Path,
    mut execute: impl FnMut(&[String]) -> Result<ExitStatus>,
) -> Result<()> {
    let clean = ["+nightly", "llvm-cov", "clean", "--workspace"].map(str::to_owned);
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
