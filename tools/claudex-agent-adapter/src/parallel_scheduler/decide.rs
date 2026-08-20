use std::time::Instant;

use crate::anthropic::{MessagesRequest, acknowledged_background_launch_count};

use super::{ParallelScheduler, SchedulerDecision, core, policy};

impl ParallelScheduler {
    pub(crate) fn decision_for_request(&self, request: &MessagesRequest) -> SchedulerDecision {
        let now = Instant::now();
        let snapshot = core::analyze_subagent_work(&request.messages);
        let key = core::thread_key(request);
        let config = self.config();
        let target_workers = policy::estimate_target_workers(&snapshot, request, &config);

        let (completed_recently, previous_last_reassessed) =
            self.previous_state(&key, &snapshot, now);
        let mut decision = SchedulerDecision {
            target_workers,
            active_workers: snapshot.active_count(),
            completed_recently,
            active_model_families: snapshot.active_model_families(),
            ..SchedulerDecision::no_action()
        };

        let mut inner = self.inner.lock().expect("parallel scheduler state");
        let should_reassess = policy::reassessment_due(&inner, &key, now, &config);
        policy::apply_reassessment_actions(
            &mut decision,
            &snapshot,
            request,
            &config,
            should_reassess,
        );
        let launch_ack_met_target = acknowledged_background_launch_count(request)
            .is_some_and(|count| count >= decision.target_workers);
        if !launch_ack_met_target {
            policy::apply_replenishment_target(
                &mut decision,
                &snapshot,
                request,
                &config,
                should_reassess,
            );
        }
        let effective_target = decision.target_workers;
        policy::apply_capacity_actions(
            &mut decision,
            effective_target,
            &config,
            policy::skip_floor_on_launch_ack(request),
        );
        policy::apply_floor_action(&mut decision, request, &config);
        policy::apply_diversity_action(&mut decision, request, &config);
        policy::apply_reuse_actions(&mut decision, request, &config);
        policy::clear_empty_decision(&mut decision, &snapshot);
        policy::persist_thread(
            &mut inner,
            key,
            now,
            should_reassess,
            previous_last_reassessed,
            snapshot.active_unit_ids,
        );
        decision
    }

    fn previous_state(
        &self,
        key: &str,
        snapshot: &core::SubagentSnapshot,
        now: Instant,
    ) -> (usize, Instant) {
        let inner = self.inner.lock().expect("parallel scheduler state");
        let Some(previous) = inner.threads.get(key) else {
            return (0, now);
        };
        let completed = core::previous_completed(
            previous,
            &snapshot.active_unit_ids,
            previous.active_units.len(),
        );
        (completed, previous.last_reassessed)
    }

    pub(crate) fn guidance_for_request(&self, request: &MessagesRequest) -> String {
        let decision = self.decision_for_request(request);
        let config = self.config();
        format!(
            "{}\n{}",
            policy::scope_guidance(request, &decision),
            decision.guidance(&config)
        )
    }
}
