use std::{
    ffi::OsString,
    num::NonZeroU64,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::anthropic::{LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV, SUBAGENT_HARD_TIMEOUT_ENV};

pub(super) fn resolve(cli: Option<NonZeroU64>) -> Result<Option<NonZeroU64>> {
    let resolution = resolve_from(
        cli,
        std::env::var_os(SUBAGENT_HARD_TIMEOUT_ENV),
        std::env::var_os(LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV),
    )?;
    if resolution.legacy_used {
        eprintln!(
            "claudex: warning: {LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV} is deprecated; use {SUBAGENT_HARD_TIMEOUT_ENV}"
        );
    }
    Ok(resolution.seconds)
}

#[derive(Debug, Eq, PartialEq)]
struct Resolution {
    seconds: Option<NonZeroU64>,
    legacy_used: bool,
}

fn resolve_from(
    cli: Option<NonZeroU64>,
    current: Option<OsString>,
    legacy: Option<OsString>,
) -> Result<Resolution> {
    let current = parse_environment(current, SUBAGENT_HARD_TIMEOUT_ENV)?;
    let legacy_used = legacy.is_some();
    let legacy = parse_environment(legacy, LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV)?;
    if current.is_some() && legacy.is_some() && current != legacy {
        bail!("{SUBAGENT_HARD_TIMEOUT_ENV} conflicts with {LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV}");
    }
    let environment = current.or(legacy);
    if cli.is_some() && environment.is_some() && cli != environment {
        bail!("--subagent-hard-timeout-seconds conflicts with the configured timeout environment");
    }
    let seconds = cli.or(environment);
    validate_timer_range(seconds)?;
    Ok(Resolution {
        seconds,
        legacy_used,
    })
}

fn validate_timer_range(seconds: Option<NonZeroU64>) -> Result<()> {
    let Some(seconds) = seconds else {
        return Ok(());
    };
    if Instant::now()
        .checked_add(Duration::from_secs(seconds.get()))
        .is_none()
    {
        bail!(
            "configured SubAgent hard timeout of {} seconds exceeds this platform's timer range",
            seconds.get()
        );
    }
    Ok(())
}

fn parse_environment(value: Option<OsString>, name: &str) -> Result<Option<NonZeroU64>> {
    value
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))?
                .parse::<NonZeroU64>()
                .with_context(|| format!("{name} must be a positive integer"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(number: u64) -> Option<NonZeroU64> {
        NonZeroU64::new(number)
    }

    #[test]
    fn resolves_unset_current_legacy_and_matching_values() {
        assert_eq!(
            resolve_from(None, None, None).unwrap(),
            Resolution {
                seconds: None,
                legacy_used: false
            }
        );
        assert_eq!(
            resolve_from(None, Some("7".into()), None).unwrap().seconds,
            value(7)
        );
        let legacy = resolve_from(None, None, Some("8".into())).unwrap();
        assert_eq!(legacy.seconds, value(8));
        assert!(legacy.legacy_used);
        assert_eq!(
            resolve_from(value(9), Some("9".into()), Some("9".into()))
                .unwrap()
                .seconds,
            value(9)
        );
    }

    #[test]
    fn rejects_invalid_and_conflicting_sources() {
        for invalid in ["0", "-1", "invalid", "18446744073709551616"] {
            assert!(resolve_from(None, Some(invalid.into()), None).is_err());
        }
        assert!(resolve_from(None, Some("7".into()), Some("8".into())).is_err());
        assert!(resolve_from(value(7), Some("8".into()), None).is_err());
    }

    #[test]
    fn validates_normalized_values_against_the_platform_timer_range() {
        let representable = value(60);
        assert_eq!(
            resolve_from(representable, None, None).unwrap().seconds,
            representable
        );
        assert_eq!(
            resolve_from(None, Some("60".into()), None).unwrap().seconds,
            representable
        );

        let maximum = value(u64::MAX);
        let platform_accepts_maximum = Instant::now()
            .checked_add(Duration::from_secs(u64::MAX))
            .is_some();
        assert_eq!(
            resolve_from(maximum, None, None).is_ok(),
            platform_accepts_maximum
        );
        assert_eq!(
            resolve_from(None, Some(u64::MAX.to_string().into()), None).is_ok(),
            platform_accepts_maximum
        );
    }
}
