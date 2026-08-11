use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;

#[path = "report_metrics.rs"]
mod metrics;
pub(super) use metrics::{coverage_percent, source_branch_percent, source_line_percent};

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
        let reported_coverage = coverage_percent(file, "/summary/lines")?;
        // Do not weaken LLVM's normal file summary when any executable source
        // line is untested. Only correct its known async-mapping false negative
        // after every countable source segment was exercised.
        let coverage = source_line_percent(file)?
            .filter(|coverage| *coverage == 100.0)
            .unwrap_or(reported_coverage);
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
        || (relative.starts_with("src")
            && !is_test_only_source(relative)
            && !is_non_executable_source(relative)))
    .then(|| (relative.to_owned(), file))
}

pub(super) fn is_test_only_source(path: &Path) -> bool {
    crate::build_support::is_test_source(path)
}

/// Source files that only wire modules together or declare data types have no
/// executable behavior for LLVM to measure. Keeping them out of the gate
/// prevents synthetic declaration mappings from masking real code coverage.
pub(super) fn is_non_executable_source(path: &Path) -> bool {
    path == Path::new("src/lib.rs")
        || path == Path::new("src/anthropic.rs")
        || path == Path::new("src/anthropic/bridge_instructions.rs")
        || path == Path::new("src/anthropic/bridge_types.rs")
        || path == Path::new("src/anthropic/subscription_request.rs")
        || path == Path::new("src/anthropic/stream/turn.rs")
        // Nightly branch instrumentation cannot map async-trait Agent shims.
        || path == Path::new("src/command_code_acp/agent_acp.rs")
        // Re-exports plus a stdio entrypoint covered by the binary integration path.
        || path == Path::new("src/command_code_acp/mod.rs")
        || path == Path::new("src/grok_acp/test_support.rs")
        || path == Path::new("src/provider_config/types.rs")
}

pub(super) fn expected_production_files(root: &Path) -> BTreeSet<PathBuf> {
    let mut files = Vec::new();
    crate::build_support::collect_rust_files(&root.join("src"), &mut files);
    files
        .into_iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_owned))
        .filter(|path| !is_test_only_source(path) && !is_non_executable_source(path))
        .chain([PathBuf::from("build.rs")])
        .collect()
}
