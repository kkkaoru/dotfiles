//! Routing hook runtime and refresh-worker implementation.

use crate::process::Deadline;
use crate::{
    Arguments, HookEvent, collect, config, exhaustion, hook, refresh, routing, snapshot, util,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct RuntimeState {
    paths: config::Paths,
    config_path: PathBuf,
    disabled_path: PathBuf,
    config: config::Config,
    live: exhaustion::LiveState,
    disabled: BTreeSet<String>,
    cache_path: PathBuf,
    key: String,
    now: f64,
    ttl: i64,
    refresh_deadline: Option<Deadline>,
}

struct CacheTarget<'a> {
    path: &'a Path,
    now: f64,
    ttl: i64,
    key: &'a str,
}

fn now_seconds() -> Result<f64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_secs_f64())
}

fn system_time(seconds: f64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs_f64(seconds.max(0.0))
}

fn merge_disabled_models(
    mut models: BTreeSet<String>,
    resolved: Option<String>,
    extra: Option<String>,
) -> Result<BTreeSet<String>> {
    if let Some(resolved) = resolved {
        models.extend(
            config::parse_environment_models(&resolved).context(
                "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS contains an invalid model ID",
            )?,
        );
    }
    if let Some(extra) = extra {
        models.extend(config::parse_environment_models(&extra)?);
    }
    Ok(models)
}

fn resolve(arguments: &Arguments, refresh_deadline: Option<Deadline>) -> Result<RuntimeState> {
    let paths = config::Paths::discover(arguments.config.as_deref())?;
    let config_path = config::provider_config_path(arguments.config.as_deref(), &paths);
    let config = config::load_config(&config_path).with_context(|| {
        format!(
            "claudex routing configuration error: {}",
            config_path.display()
        )
    })?;
    let disabled_path =
        config::disabled_models_path(arguments.disabled_models_config.as_deref(), &paths)?;
    let configured = merge_disabled_models(
        config::load_disabled_models(&disabled_path)?,
        env::var("CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS").ok(),
        env::var("CLAUDEX_DISABLED_SUBAGENT_MODELS").ok(),
    )
    .context("claudex routing configuration error")?;
    routing::orchestration::orchestration_settings()
        .context("claudex routing configuration error")?;
    let now = now_seconds()?;
    let live = exhaustion::live_state(&paths.home, system_time(now));
    let disabled = exhaustion::effective_disabled_models(&config, &configured, &live);
    let key = config::configuration_key(&config.raw, &disabled);
    let cache_path = paths.home.join(".cache/claudex/usage-routing.json");
    Ok(RuntimeState {
        paths,
        config_path,
        disabled_path,
        config,
        live,
        disabled,
        cache_path,
        key,
        now,
        ttl: util::cache_seconds(),
        refresh_deadline,
    })
}

fn usage_report(arguments: &Arguments, state: &RuntimeState) -> Result<Value> {
    let Some(path) = &arguments.input else {
        return Ok(Value::Array(collect::collect_usage_before(
            &state.config,
            &arguments.codexbar_program,
            &arguments.curl_program,
            &state.paths,
            state.now,
            &state.disabled,
            state.refresh_deadline,
        )));
    };
    Ok(serde_json::from_str::<Value>(&std::fs::read_to_string(
        path,
    )?)?)
}

fn build_summary(arguments: &Arguments, state: &RuntimeState) -> Result<Value> {
    usage_report(arguments, state)
        .and_then(|report| {
            routing::summary::routing_summary_with_exhaustion(
                &report,
                &state.config,
                &state.disabled,
                &state.live.scopes,
                state.live.codex_backend_cooling,
            )
        })
        .or_else(|_| {
            routing::summary::fallback_summary("usage-unavailable", &state.config, &state.disabled)
        })
}

fn synchronous(arguments: &Arguments, payload: Option<&Value>) -> bool {
    arguments.no_cache || arguments.input.is_some() || payload.is_none()
}

fn cached_or_built(
    arguments: &Arguments,
    state: &RuntimeState,
    target: &CacheTarget<'_>,
) -> Result<Value> {
    if let Some(summary) = util::read_routing_cache(target.path, target.now, target.ttl, target.key)
    {
        return Ok(summary);
    }
    let summary = build_summary(arguments, state)?;
    if target.ttl > 0 {
        let _ = refresh::publish_sync(target.path, &summary, target.now, target.key)?;
    }
    Ok(summary)
}

fn fast_snapshot(arguments: &Arguments, state: &RuntimeState) -> Result<Value> {
    let (summary, fresh) = snapshot::last_known_good_or_else(
        &state.cache_path,
        &state.key,
        &state.disabled,
        state.now,
        state.ttl,
        || {
            routing::summary::fallback_summary(
                "usage-snapshot-missing",
                &state.config,
                &state.disabled,
            )
        },
    )?;
    let request = refresh::SpawnRequest {
        cache_path: &state.cache_path,
        home: &state.paths.home,
        config_path: &state.config_path,
        disabled_path: &state.disabled_path,
        codexbar_program: &arguments.codexbar_program,
        curl_program: &arguments.curl_program,
        configuration_key: &state.key,
    };
    let _ = refresh::schedule(&request, fresh);
    Ok(summary)
}

fn apply_live_state(
    arguments: &Arguments,
    state: &RuntimeState,
    mut summary: Value,
    use_processes: bool,
) -> Result<Value> {
    let diagnostic_deadline = use_processes.then(|| Deadline::after(Duration::from_secs(20)));
    let health = use_processes
        .then(|| collect::run_daemon_health_before(&arguments.curl_program, diagnostic_deadline))
        .flatten();
    summary = routing::concurrency::apply_model_concurrency_with_inflight(
        summary,
        &state.config,
        health.as_ref().map(|value| &value.model_concurrency),
        health
            .as_ref()
            .map(|value| &value.active_subagent_models)
            .unwrap_or(&BTreeMap::new()),
        &state.disabled,
    )?;
    let main_model = env::var("CLAUDEX_MAIN_MODEL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var("CLAUDEX_OUTER_MODEL")
                .ok()
                .filter(|value| !value.is_empty())
        });
    let main_known = util::boolean_env("CLAUDEX_MAIN_MODEL_KNOWN", main_model.is_some())?;
    let allow_sonnet = util::boolean_env("CLAUDEX_ALLOW_SONNET_SUBAGENT", false)?;
    summary = routing::workers::enforce_worker_model_separation(
        summary,
        main_model.as_deref(),
        main_known,
        allow_sonnet,
    )?;
    let Some(object) = summary.as_object_mut() else {
        bail!("routing summary must be an object");
    };
    object.insert(
        "memory_status".into(),
        if use_processes {
            collect::read_memory_status_before(diagnostic_deadline)
        } else {
            serde_json::json!({
                "status": "unavailable",
                "reason": "last-known-good-fast-path"
            })
        },
    );
    Ok(summary)
}

pub fn normal_hook_output(arguments: &Arguments, payload: Option<&Value>) -> Result<Value> {
    let agent_type = payload.and_then(hook::agent_type_from_payload);
    if arguments.event == HookEvent::SubagentStart && hook::is_command_code_agent(agent_type) {
        return Ok(hook::slim_command_code_hook(arguments.event.as_str()));
    }
    let state = resolve(arguments, None)?;
    let use_processes = synchronous(arguments, payload);
    let summary = if use_processes {
        let target = CacheTarget {
            path: &state.cache_path,
            now: state.now,
            ttl: if arguments.no_cache || arguments.input.is_some() {
                0
            } else {
                state.ttl
            },
            key: &state.key,
        };
        cached_or_built(arguments, &state, &target)?
    } else {
        fast_snapshot(arguments, &state)?
    };
    let summary = apply_live_state(arguments, &state, summary, use_processes)?;
    let _ = util::write_delegation_state(&state.paths.home, &summary, state.now);
    hook::hook_output_for_agent(&summary, arguments.event.as_str(), agent_type)
}

pub fn refresh_cache_worker(arguments: &Arguments) -> Result<()> {
    refresh_cache_worker_inner(arguments, Deadline::after(refresh::WORKER_TIMEOUT))
}

fn refresh_cache_worker_inner(arguments: &Arguments, deadline: Deadline) -> Result<()> {
    let (Some(lock_fd), Some(ticket_fd)) = (arguments.refresh_lock_fd, arguments.refresh_ticket_fd)
    else {
        return Ok(());
    };
    let Some(guard) = refresh::claim_worker(lock_fd, ticket_fd) else {
        return Ok(());
    };
    deadline.check("refresh configuration resolution")?;
    let state = resolve(arguments, Some(deadline))?;
    if !guard.owns_configuration(&state.key) {
        return Ok(());
    }
    let summary = build_summary(arguments, &state)?;
    deadline.check("routing summary collection")?;
    let publish_state = resolve(arguments, None)?;
    deadline.check("routing cache policy revalidation")?;
    if !guard.owns_configuration(&publish_state.key) || state.key != publish_state.key {
        return Ok(());
    }
    let _ = refresh::publish_worker(
        &guard,
        &publish_state.cache_path,
        &summary,
        publish_state.now,
        &publish_state.key,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_disabled_snapshot_is_unioned_with_live_and_extra_models() {
        let live = BTreeSet::from(["qwen3.8-max-preview".to_owned()]);
        let merged = merge_disabled_models(
            live,
            Some("grok-4.5,opencode-go/deepseek-v4-flash".to_owned()),
            Some("opencode-go/gpt-5.6-luna".to_owned()),
        )
        .expect("merge disabled model sources");
        assert_eq!(
            merged,
            BTreeSet::from([
                "grok-4.5".to_owned(),
                "opencode-go/deepseek-v4-flash".to_owned(),
                "opencode-go/gpt-5.6-luna".to_owned(),
                "qwen3.8-max-preview".to_owned(),
            ])
        );
    }
}
