use super::{SchedulerConfig, SchedulerDecision};

impl SchedulerDecision {
    pub(super) fn no_action() -> Self {
        Self {
            target_workers: 0,
            active_workers: 0,
            completed_recently: 0,
            active_model_families: 0,
            needs_more_workers: 0,
            needs_model_diversity: false,
            active_floor_breached: false,
            actions: Vec::new(),
        }
    }

    pub(super) fn has_work(&self) -> bool {
        self.active_workers > 0
    }

    pub(super) fn guidance(&self, config: &SchedulerConfig) -> String {
        if self.actions.is_empty() {
            return format!(
                "Dynamic parallel status: keep {} active lane(s) for the current independent work; re-check after each SubAgent completion or every {} minutes.",
                self.target_workers,
                config.reassess_interval.as_secs().div_ceil(60).max(1)
            );
        }
        let mut lines = Vec::with_capacity(1 + self.actions.len() + 2);
        lines.push(format!(
            "SubAgent floor policy: target concurrency is {} matching independent scopes; a single scope stays at one worker.",
            self.target_workers
        ));
        if self.completed_recently > 0 {
            lines.push(format!(
                "Worker-cycle: {completed} worker(s) completed on this turnset; for unfinished objectives, continue running SubAgents with follow-up context, re-issue same-scope tasks, then add expanded follow-up scope.",
                completed = self.completed_recently
            ));
        }
        let required_families = config.min_model_families.min(self.target_workers);
        if self.target_workers >= 2 && self.active_model_families < required_families {
            lines.push(format!(
                "Model-policy: ensure at least {required_families} model families remain active."
            ));
        }
        lines.extend(
            self.actions
                .iter()
                .map(|action| format!("Action: {action}")),
        );
        lines.push(format!(
            "Current active lanes: {} (completed recently: {}).",
            self.active_workers, self.completed_recently
        ));
        lines.join("\n")
    }
}
