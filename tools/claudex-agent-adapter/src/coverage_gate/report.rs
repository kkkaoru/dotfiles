use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;

const MINIMUM_PERCENT: f64 = 95.0;

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
