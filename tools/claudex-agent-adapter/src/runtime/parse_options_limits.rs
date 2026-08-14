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

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    use super::*;

    fn arguments(values: &[&str]) -> VecDeque<OsString> {
        values.iter().copied().map(OsString::from).collect()
    }

    #[test]
    fn applies_each_subscription_limit() {
        let mut draft = OptionsDraft::default();
        let mut max_processes = arguments(&["--subscription-max-processes", "7"]);
        apply_limit_option(
            &mut max_processes,
            "--subscription-max-processes",
            &mut draft,
        )
        .expect("max process limit");
        assert_eq!(draft.max_processes, 7);

        let mut timeout_minutes = arguments(&["--subscription-timeout-minutes", "9"]);
        apply_limit_option(
            &mut timeout_minutes,
            "--subscription-timeout-minutes",
            &mut draft,
        )
        .expect("timeout limit");
        assert_eq!(draft.timeout_minutes, 9);
    }

    #[test]
    #[should_panic(expected = "limit option filter")]
    fn rejects_an_unknown_limit_option_at_the_internal_boundary() {
        let mut draft = OptionsDraft::default();
        let mut values = arguments(&["--unknown", "1"]);
        let _ = apply_limit_option(&mut values, "--unknown", &mut draft);
    }

    #[test]
    fn parses_and_rejects_hard_timeout_values() {
        let mut timeout = None;
        let mut values = arguments(&["--subagent-hard-timeout-seconds", "17"]);
        parse_hard_timeout(&mut values, "--subagent-hard-timeout-seconds", &mut timeout)
            .expect("hard timeout");
        assert_eq!(timeout.map(std::num::NonZeroU64::get), Some(17));

        let mut duplicate = arguments(&["--subagent-hard-timeout-seconds", "18"]);
        let error = parse_hard_timeout(
            &mut duplicate,
            "--subagent-hard-timeout-seconds",
            &mut timeout,
        )
        .expect_err("duplicate hard timeout");
        assert!(error.to_string().contains("must not be repeated"));

        let mut zero = arguments(&["--subagent-hard-timeout-seconds", "0"]);
        let mut unset = None;
        let error = parse_hard_timeout(&mut zero, "--subagent-hard-timeout-seconds", &mut unset)
            .expect_err("zero hard timeout");
        assert!(error.to_string().contains("positive integer"));
    }

    #[test]
    fn reports_missing_and_non_utf8_option_values() {
        let mut missing = arguments(&["--model"]);
        let error = option_value(&mut missing, "--model").expect_err("missing value");
        assert!(
            error
                .to_string()
                .contains("value for adapter option --model is required")
        );

        #[cfg(unix)]
        {
            let mut invalid =
                VecDeque::from([OsString::from("--model"), OsString::from_vec(vec![0xff])]);
            let error = option_value(&mut invalid, "--model").expect_err("UTF-8 value");
            assert!(error.to_string().contains("must be valid UTF-8"));
        }
    }

    #[test]
    fn rejects_non_positive_numbers() {
        let mut zero = arguments(&["--subscription-max-processes", "0"]);
        let error = positive_number::<usize>(&mut zero, "--subscription-max-processes")
            .expect_err("zero must fail");
        assert!(error.to_string().contains("positive integer"));

        let mut invalid = arguments(&["--subscription-max-processes", "not-a-number"]);
        let error = positive_number::<usize>(&mut invalid, "--subscription-max-processes")
            .expect_err("invalid number must fail");
        assert!(error.to_string().contains("positive integer"));
    }

    #[test]
    fn utf8_requires_a_value_and_rejects_invalid_bytes() {
        let error = utf8(None, "model").expect_err("missing UTF-8 value");
        assert!(error.to_string().contains("model is required"));

        #[cfg(unix)]
        {
            let error = utf8(Some(OsString::from_vec(vec![0xff])), "model")
                .expect_err("invalid UTF-8 value");
            assert!(error.to_string().contains("model must be valid UTF-8"));
        }
    }
}
