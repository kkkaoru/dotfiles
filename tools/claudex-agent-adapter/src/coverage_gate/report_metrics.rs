use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use serde_json::Value;

use super::{BranchKey, production_file};

pub(in crate::coverage_gate) fn coverage_percent(value: &Value, pointer: &str) -> Result<f64> {
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

/// Calculate file line coverage from mapped executable source segments.
///
/// LLVM's file summary can count an async state-machine mapping that has no
/// source counter. Restricting this check to `has_count` segments prevents that
/// synthetic mapping from making a fully exercised source file appear uncovered.
pub(in crate::coverage_gate) fn source_line_percent(file: &Value) -> Result<Option<f64>> {
    let Some(segments) = file.get("segments") else {
        return Ok(None);
    };
    let segments = segments
        .as_array()
        .context("llvm-cov file segments are not an array")?;
    let mut lines = BTreeMap::<u64, bool>::new();
    for segment in segments {
        let values = segment
            .as_array()
            .context("llvm-cov source segment is not an array")?;
        let line = segment_value(values, 0, "line")?;
        let count = segment_value(values, 2, "count")?;
        let has_count = values
            .get(3)
            .and_then(Value::as_bool)
            .context("llvm-cov source segment is missing has_count")?;
        if has_count {
            lines
                .entry(line)
                .and_modify(|covered| *covered |= count > 0)
                .or_insert(count > 0);
        }
    }
    let Some(total) = u64::try_from(lines.len()).ok().filter(|total| *total > 0) else {
        return Ok(None);
    };
    let covered = lines.values().filter(|covered| **covered).count() as u64;
    Ok(Some(covered as f64 * 100.0 / total as f64))
}

fn segment_value(values: &[Value], index: usize, name: &str) -> Result<u64> {
    values
        .get(index)
        .and_then(Value::as_u64)
        .with_context(|| format!("llvm-cov source segment is missing {name}"))
}

pub(in crate::coverage_gate) fn source_branch_percent(root: &Path, data: &Value) -> Result<f64> {
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
            let key = (
                path.clone(),
                branch_record_number(values, 0)?,
                branch_record_number(values, 1)?,
                branch_record_number(values, 2)?,
                branch_record_number(values, 3)?,
            );
            let counts = branches.entry(key).or_default();
            counts.0 = counts.0.saturating_add(branch_record_number(values, 4)?);
            counts.1 = counts.1.saturating_add(branch_record_number(values, 5)?);
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

fn branch_record_number(values: &[Value], index: usize) -> Result<u64> {
    values
        .get(index)
        .and_then(Value::as_u64)
        .context("llvm-cov branch record is incomplete")
}
