use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;

const MINIMUM_PERCENT: f64 = 95.0;

type BranchKey = (PathBuf, u64, u64, u64, u64);

pub(super) fn production_line_failures(root: &Path, data: &Value) -> Result<Vec<String>> {
    let reported = data
        .get("files")
        .and_then(Value::as_array)
        .context("llvm-cov report has no files")?
        .iter()
        .filter_map(|file| production_file(root, file))
        .collect::<BTreeMap<_, _>>();
    let expected = expected_production_files(root);
    let reported_paths = reported.keys().cloned().collect::<BTreeSet<_>>();
    let mut failures = expected
        .difference(&reported_paths)
        .map(|path| format!("{}: missing from report", path.display()))
        .chain(
            reported_paths
                .difference(&expected)
                .map(|path| format!("{}: unexpected production file", path.display())),
        )
        .collect::<Vec<_>>();
    for (path, file) in reported {
        let coverage = coverage_percent(file, "/summary/lines")?;
        if coverage < MINIMUM_PERCENT {
            failures.push(format!("{}: {coverage:.2}%", path.display()));
        }
    }
    Ok(failures)
}

pub(super) fn production_file<'a>(root: &Path, file: &'a Value) -> Option<(PathBuf, &'a Value)> {
    let path = PathBuf::from(file.get("filename")?.as_str()?);
    let relative = path.strip_prefix(root).ok()?;
    (relative == Path::new("build.rs")
        || (relative.starts_with("src") && !is_test_only_source(relative)))
    .then(|| (relative.to_owned(), file))
}

pub(super) fn is_test_only_source(path: &Path) -> bool {
    crate::build_support::is_test_source(path)
}

pub(super) fn expected_production_files(root: &Path) -> BTreeSet<PathBuf> {
    let mut files = Vec::new();
    crate::build_support::collect_rust_files(&root.join("src"), &mut files);
    files
        .into_iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_owned))
        .filter(|path| !is_test_only_source(path))
        .chain([PathBuf::from("build.rs")])
        .collect()
}

pub(super) fn coverage_percent(value: &Value, pointer: &str) -> Result<f64> {
    let coverage = value
        .pointer(pointer)
        .with_context(|| format!("llvm-cov report is missing {pointer}"))?;
    let covered = coverage
        .get("covered")
        .and_then(Value::as_u64)
        .with_context(|| format!("llvm-cov report is missing {pointer}/covered"))?;
    let count = coverage
        .get("count")
        .and_then(Value::as_u64)
        .with_context(|| format!("llvm-cov report is missing {pointer}/count"))?;
    Ok(if count == 0 {
        100.0
    } else {
        covered as f64 * 100.0 / count as f64
    })
}

pub(super) fn source_branch_percent(root: &Path, data: &Value) -> Result<f64> {
    let files = data
        .get("files")
        .and_then(Value::as_array)
        .context("llvm-cov report has no files")?;
    let mut branches = BTreeMap::<BranchKey, (u64, u64)>::new();
    for (path, file) in files.iter().filter_map(|file| production_file(root, file)) {
        let Some(records) = file.get("branches").and_then(Value::as_array) else {
            continue;
        };
        for record in records {
            let values = record
                .as_array()
                .context("llvm-cov branch record is not an array")?;
            let number = |index| {
                values
                    .get(index)
                    .and_then(Value::as_u64)
                    .context("llvm-cov branch record is incomplete")
            };
            let key = (path.clone(), number(0)?, number(1)?, number(2)?, number(3)?);
            let counts = branches.entry(key).or_default();
            counts.0 = counts.0.saturating_add(number(4)?);
            counts.1 = counts.1.saturating_add(number(5)?);
        }
    }
    if branches.is_empty() {
        return coverage_percent(data, "/totals/branches");
    }
    let covered = branches
        .values()
        .map(|(taken, skipped)| u64::from(*taken > 0) + u64::from(*skipped > 0))
        .sum::<u64>();
    Ok(covered as f64 * 100.0 / (branches.len() * 2) as f64)
}
