//! One absolute wall-clock bound shared by collection and cleanup.

use anyhow::{Result, bail};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Deadline {
    hard: Instant,
}

impl Deadline {
    pub(crate) fn after(total: Duration) -> Self {
        Self {
            hard: Instant::now() + total,
        }
    }

    pub(crate) fn instant(self) -> Instant {
        self.hard
    }

    pub(crate) fn remaining(self) -> Option<Duration> {
        self.hard.checked_duration_since(Instant::now())
    }

    pub(crate) fn check(self, stage: &str) -> Result<()> {
        if self
            .remaining()
            .is_some_and(|remaining| !remaining.is_zero())
        {
            Ok(())
        } else {
            bail!("absolute subprocess deadline expired during {stage}")
        }
    }

    pub(crate) fn cutoff(self, cap: Duration, reserve: Duration) -> Option<Instant> {
        let now = Instant::now();
        let latest = self.hard.checked_sub(reserve)?;
        (now < latest).then(|| (now + cap).min(latest))
    }
}
