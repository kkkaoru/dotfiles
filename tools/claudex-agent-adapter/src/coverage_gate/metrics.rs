use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{coverage_percent, source_branch_percent};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub(super) struct CoverageMetrics {
    pub(super) lines: f64,
    pub(super) functions: f64,
    pub(super) regions: f64,
    pub(super) branches: f64,
}

impl CoverageMetrics {
    pub(super) fn from_report(root: &Path, data: &Value) -> Result<Self> {
        Ok(Self {
            lines: coverage_percent(data, "/totals/lines")?,
            functions: coverage_percent(data, "/totals/functions")?,
            regions: coverage_percent(data, "/totals/regions")?,
            branches: source_branch_percent(root, data)?,
        })
    }

    fn below(&self, baseline: &Self) -> Vec<String> {
        [
            ("lines", self.lines, baseline.lines),
            ("functions", self.functions, baseline.functions),
            ("regions", self.regions, baseline.regions),
            ("branches", self.branches, baseline.branches),
        ]
        .into_iter()
        .filter(|(_, actual, expected)| actual < expected)
        .map(|(name, actual, expected)| format!("{name}: {actual:.2}% < {expected:.2}%"))
        .collect()
    }
}

pub(super) fn enforce_baseline(root: &Path, metrics: &CoverageMetrics) -> Result<()> {
    let baseline_path = root.join("coverage-baseline.json");
    let Ok(contents) = fs::read(&baseline_path) else {
        return Ok(());
    };
    let baseline: CoverageMetrics = serde_json::from_slice(&contents)
        .with_context(|| format!("invalid {}", baseline_path.display()))?;
    let failures = metrics.below(&baseline);
    if failures.is_empty() || drop_is_allowed(root) {
        return Ok(());
    }
    bail!(
        "coverage dropped below baseline (set CLAUDEX_COVERAGE_ALLOW_DROP=1 or create coverage-baseline.allow to override):\n{}",
        failures.join("\n")
    )
}

pub(super) fn persist(root: &Path, report: &Path, metrics: &CoverageMetrics) -> Result<()> {
    let directory = root.join("target/coverage-last");
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let retained_report = directory.join("branch-coverage.json");
    if report != retained_report {
        fs::copy(report, &retained_report)
            .with_context(|| format!("failed to retain {}", report.display()))?;
    }
    let path = directory.join("metrics.json");
    fs::write(&path, serde_json::to_vec_pretty(metrics)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn drop_is_allowed(root: &Path) -> bool {
    std::env::var_os("CLAUDEX_COVERAGE_ALLOW_DROP").is_some()
        || root.join("coverage-baseline.allow").is_file()
}
