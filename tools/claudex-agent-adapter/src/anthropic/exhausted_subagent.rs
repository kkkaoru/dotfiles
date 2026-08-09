use anyhow::Result;

use crate::agent_backend::BackendKind;

use super::{Bridge, MessagesRequest, model_concurrency::Ticket, request_routing::RouteDecision};

pub(super) fn subagent_failover_source_ok(kind: BackendKind) -> bool {
    matches!(
        kind,
        BackendKind::ConfiguredAcp
            | BackendKind::GrokAcp
            | BackendKind::CopilotAcp
            | BackendKind::CodexAppServer
    )
}

pub(super) fn subagent_failover_target_ok(kind: BackendKind) -> bool {
    matches!(
        kind,
        BackendKind::ConfiguredAcp | BackendKind::GrokAcp | BackendKind::CopilotAcp
    )
}

impl Bridge {
    /// After Cline empty-ACP cooldown, a nested Agent still hydrates the stale
    /// `selected_workers` snapshot. Rewrite that launch onto the sibling provider
    /// before validation, including the catalog worker type so exact-route passes.
    pub(super) fn rewrite_exhausted_agent_launch_with_quota(
        &self,
        arguments: &mut serde_json::Value,
        messages: &[serde_json::Value],
        system: &serde_json::Value,
    ) {
        let Some(model) = super::agent_effort::requested_model(arguments).map(str::to_owned) else {
            return;
        };
        let quota = super::agent_routing::active_routing_summary(messages, system);
        if !self.subagent_model_is_exhausted(&model, quota.as_ref()) {
            return;
        }
        let Some(failover) = self.subagent_provider_failover_excluding(&model, quota.as_ref())
        else {
            return;
        };
        let agent = self
            .model_catalog
            .worker_agent_for_model(&failover.model)
            .map(str::to_owned);
        let Some(object) = arguments.as_object_mut() else {
            return;
        };
        if let Some(agent) = agent {
            object.insert("subagent_type".to_owned(), serde_json::Value::String(agent));
        }
        tracing::info!(
            exhausted_model = %model,
            failover_model = %failover.model,
            "rewrote exhausted SubAgent launch onto a sibling provider"
        );
        object.insert(
            "claudex_model".to_owned(),
            serde_json::Value::String(failover.model),
        );
        if let Some(effort) = failover.effort {
            object.insert(
                "claudex_effort".to_owned(),
                serde_json::Value::String(effort),
            );
        } else {
            object.remove("claudex_effort");
        }
    }

    /// Outer Subscription → explicit Cline SubAgent HTTP still lands here after
    /// hydrate. Rewrite onto a sibling Provider instead of 502 cooldown reject.
    pub(super) fn rewrite_exhausted_subagent_request(
        &self,
        request: &mut MessagesRequest,
        route: RouteDecision,
        effort: &mut Option<String>,
        is_subagent: bool,
    ) -> Result<RouteDecision> {
        let quota =
            super::agent_routing::active_routing_summary(&request.messages, &request.system);
        if !is_subagent || !self.subagent_model_is_exhausted(&request.model, quota.as_ref()) {
            return Ok(route);
        }
        let Some(failover) =
            self.subagent_provider_failover_excluding(&request.model, quota.as_ref())
        else {
            return Err(anyhow::anyhow!(
                "provider for model `{}` is cooling down after rate/usage/billing limit; orchestrator should re-route",
                request.model
            ));
        };
        tracing::info!(
            exhausted_model = %request.model,
            failover_model = %failover.model,
            "rewrote exhausted SubAgent request onto a sibling provider"
        );
        request.model = failover.model;
        if let Some(failover_effort) = failover.effort {
            *effort = Some(failover_effort);
        }
        Ok(failover.route)
    }

    /// Qwen `maxConcurrency: 3` only admits two SubAgent slots. A third hop,
    /// including Cline empty-ACP → Qwen, used to wait 30s then surface
    /// `concurrency model admission timed out` to Claude Code.
    pub(super) fn apply_concurrency_preflight(
        &self,
        request: &mut MessagesRequest,
        route: RouteDecision,
        effort: &mut Option<String>,
        is_subagent: bool,
    ) -> RouteDecision {
        if !is_subagent || route != RouteDecision::Provider {
            return route;
        }
        if !self
            .model_concurrency
            .is_subagent_at_capacity(&request.model)
        {
            return route;
        }
        let Some(failover) = self.subagent_provider_failover_for(&request.model) else {
            tracing::warn!(
                model = %request.model,
                "SubAgent model is at concurrency capacity but no sibling provider is free"
            );
            return route;
        };
        tracing::warn!(
            saturated_model = %request.model,
            failover_model = %failover.model,
            failover_route = ?failover.route,
            "preflight failover away from saturated SubAgent model"
        );
        request.model = failover.model;
        if let Some(failover_effort) = failover.effort {
            *effort = Some(failover_effort);
        }
        failover.route
    }

    /// `None` if no sibling remains. Inner `None` means the sibling has no
    /// concurrency limit and can start without a ticket.
    pub(super) fn reticket_after_concurrency_timeout(
        &self,
        request: &mut MessagesRequest,
        effort: &mut Option<String>,
    ) -> Option<Option<Ticket>> {
        let failover = self.subagent_provider_failover_for(&request.model)?;
        tracing::warn!(
            saturated_model = %request.model,
            failover_model = %failover.model,
            "SubAgent concurrency admission timed out; retrying sibling provider"
        );
        request.model = failover.model;
        if let Some(failover_effort) = failover.effort {
            *effort = Some(failover_effort);
        }
        Some(self.model_concurrency.ticket(
            &request.model,
            self.app.max_concurrency_for_model(&request.model),
        ))
    }
}
