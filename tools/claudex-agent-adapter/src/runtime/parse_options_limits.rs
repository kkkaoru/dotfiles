use std::{collections::VecDeque, ffi::OsString};

use anyhow::{Context, Result, bail};

use super::OptionsDraft;

pub(super) fn apply_limit_option(
    arguments: &mut VecDeque<OsString>,
    option: &str,
    draft: &mut OptionsDraft,
) -> Result<()> {
    match option {
        "--subscription-max-processes" => {
            draft.max_processes = positive_number(arguments, option)?;
        }
        "--subscription-timeout-minutes" => {
            draft.timeout_minutes = positive_number(arguments, option)?;
        }
        _ => unreachable!("limit option filter"),
    }
    Ok(())
}

pub(super) fn parse_hard_timeout(
    arguments: &mut VecDeque<OsString>,
    option: &str,
    hard_timeout: &mut Option<std::num::NonZeroU64>,
) -> Result<()> {
    if hard_timeout.is_some() {
        bail!("--subagent-hard-timeout-seconds must not be repeated");
    }
    let seconds: u64 = positive_number(arguments, option)?;
    *hard_timeout = std::num::NonZeroU64::new(seconds);
    Ok(())
}

pub(super) fn option_value(arguments: &mut VecDeque<OsString>, option: &str) -> Result<String> {
    arguments.pop_front();
    utf8(
        arguments.pop_front(),
        &format!("value for adapter option {option}"),
    )
}

pub(super) fn positive_number<T>(arguments: &mut VecDeque<OsString>, option: &str) -> Result<T>
where
    T: std::str::FromStr + PartialOrd + From<u8>,
{
    let value = option_value(arguments, option)?;
    value
        .parse::<T>()
        .ok()
        .filter(|number| *number > T::from(0))
        .with_context(|| format!("{option} must be a positive integer"))
}

pub(super) fn utf8(value: Option<OsString>, name: &str) -> Result<String> {
    value
        .with_context(|| format!("{name} is required"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))
}
