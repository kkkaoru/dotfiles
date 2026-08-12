use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

mod report;
mod runner;
#[cfg(test)]
use report::INSTRUMENTATION_EXCEPTIONS;
#[cfg(test)]
use report::is_non_executable_source;
#[cfg(test)]
use report::is_test_only_source;
#[cfg(test)]
use report::source_line_percent;
use report::{coverage_percent, production_line_failures, source_branch_percent};
pub use runner::run;

const MINIMUM_PERCENT: f64 = 95.0;
const TOTAL_METRICS: [&str; 3] = ["lines", "functions", "regions"];

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
#[path = "coverage_gate/tests.rs"]
mod tests;
