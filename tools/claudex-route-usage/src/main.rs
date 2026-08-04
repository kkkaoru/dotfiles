mod collect;
mod config;
mod hook;
mod opencode_go_budget;
mod routing;
mod util;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "PascalCase")]
enum HookEvent {
    UserPromptSubmit,
    SubagentStart,
}

impl HookEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SubagentStart => "SubagentStart",
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Emit sanitized routing context from Codexbar and Qwen Cloud quota")]
struct Arguments {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    disabled_models_config: Option<PathBuf>,
    #[arg(long)]
    input: Option<PathBuf>,
    #[arg(long)]
    no_cache: bool,
    #[arg(long, default_value = "codexbar")]
    codexbar_program: String,
    #[arg(long, default_value = "curl")]
    curl_program: String,
    #[arg(long, value_enum, default_value = "UserPromptSubmit")]
    event: HookEvent,
}

fn now_seconds() -> Result<f64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_secs_f64())
}

fn is_internal_notification_prompt(prompt: &str) -> bool {
    let text = prompt.trim();
    if (text.starts_with("<agent-message") && text.contains("</agent-message>"))
        || (text.starts_with("<task-notification") && text.contains("</task-notification>"))
    {
        return true;
    }
    let Some(remainder) = text.strip_prefix("Another Claude session sent a message:") else {
        return false;
    };
    let remainder = remainder.trim_start();
    (remainder.starts_with("<agent-message") && remainder.contains("</agent-message>"))
        || (remainder.starts_with("<task-notification")
            && remainder.contains("</task-notification>"))
}

fn block_internal_notification_from_hook() -> Result<bool> {
    if env::var("CLAUDEX_ACTIVE").as_deref() != Ok("1")
        || env::var("CLAUDEX_AGMSG_AUTO_MONITOR").as_deref() == Ok("1")
        || io::stdin().is_terminal()
    {
        return Ok(false);
    }
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    if raw.is_empty() {
        return Ok(false);
    }
    let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
        return Ok(false);
    };
    let Some(prompt) = payload.get("user_prompt").and_then(Value::as_str) else {
        return Ok(false);
    };
    if !is_internal_notification_prompt(prompt) {
        return Ok(false);
    }
    println!(
        "{}",
        serde_json::json!({
            "decision": "block",
            "reason": "Claudex internal background notification consumed"
        })
    );
    Ok(true)
}

fn disabled_models(arguments: &Arguments, paths: &config::Paths) -> Result<BTreeSet<String>> {
    let explicit_config = arguments.disabled_models_config.is_some()
        || env::var_os("CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG").is_some();
    if !explicit_config {
        if let Ok(resolved) = env::var("CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS") {
            return config::parse_environment_models(&resolved)
                .context("CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS contains an invalid model ID");
        }
    }
    let path = config::disabled_models_path(arguments.disabled_models_config.as_deref(), paths)?;
    let mut models = config::load_disabled_models(&path)?;
    if let Ok(extra) = env::var("CLAUDEX_DISABLED_SUBAGENT_MODELS") {
        models.extend(config::parse_environment_models(&extra)?);
    }
    Ok(models)
}

fn run() -> Result<()> {
    let arguments = Arguments::parse();
    if block_internal_notification_from_hook()? {
        return Ok(());
    }
    let paths = config::Paths::discover(arguments.config.as_deref())?;
    let config_path = config::provider_config_path(arguments.config.as_deref(), &paths);
    let config = config::load_config(&config_path)
        .with_context(|| format!("claudex routing configuration error: {}", config_path.display()))?;
    let disabled = disabled_models(&arguments, &paths)
        .context("claudex routing configuration error")?;
    routing::orchestration_settings().context("claudex routing configuration error")?;
    let now = now_seconds()?;
    let key = config::configuration_key(&config.raw, &disabled);
    let ttl = if arguments.no_cache || arguments.input.is_some() {
        0
    } else {
        util::cache_seconds()
    };
    let cache_path = paths.home.join(".cache/claudex/usage-routing.json");
    let qwen_cache = paths.home.join(".cache/claudex/qwen-quota.json");
    let mut summary = util::read_routing_cache(&cache_path, now, ttl, &key)
        .filter(|value| !collect::qwen_quota_refresh_due(value, &config, &qwen_cache, now));
    if summary.is_none() {
        let built = match (|| -> Result<Value> {
            let report = if let Some(path) = &arguments.input {
                serde_json::from_str::<Value>(&std::fs::read_to_string(path)?)?
            } else {
                Value::Array(collect::collect_usage(
                    &config,
                    &arguments.codexbar_program,
                    &arguments.curl_program,
                    &paths,
                    now,
                    &disabled,
                ))
            };
            routing::routing_summary(&report, &config, &disabled)
        })() {
            Ok(built) => {
                if ttl > 0 {
                    util::write_routing_cache(&cache_path, &built, now, &key)?;
                }
                built
            }
            Err(_) => routing::fallback_summary("usage-unavailable", &config, &disabled)?,
        };
        summary = Some(built);
    }
    let mut summary = summary.ok_or_else(|| anyhow::anyhow!("routing summary unavailable"))?;
    let health = collect::run_daemon_health(&arguments.curl_program);
    summary = routing::apply_model_concurrency(summary, &config, health.as_ref(), &disabled)?;
    let main_model = env::var("CLAUDEX_MAIN_MODEL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| env::var("CLAUDEX_OUTER_MODEL").ok().filter(|value| !value.is_empty()));
    let main_known = util::boolean_env("CLAUDEX_MAIN_MODEL_KNOWN", main_model.is_some())?;
    let allow_sonnet = util::boolean_env("CLAUDEX_ALLOW_SONNET_SUBAGENT", false)?;
    summary = routing::enforce_worker_model_separation(
        summary,
        main_model.as_deref(),
        main_known,
        allow_sonnet,
    )?;
    if let Some(object) = summary.as_object_mut() {
        object.insert("memory_status".into(), collect::read_memory_status());
    } else {
        bail!("routing summary must be an object");
    }
    println!(
        "{}",
        serde_json::to_string(&hook::hook_output(&summary, arguments.event.as_str())?)?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::is_internal_notification_prompt;

    #[test]
    fn identifies_only_lifecycle_notifications() {
        assert!(is_internal_notification_prompt(
            "<agent-message from=\"worker\">done</agent-message>"
        ));
        assert!(is_internal_notification_prompt(
            "Another Claude session sent a message: <task-notification>done</task-notification>"
        ));
        assert!(!is_internal_notification_prompt(
            "Explain the literal <agent-message> tag"
        ));
        assert!(!is_internal_notification_prompt(
            "<teammate-message>work</teammate-message>"
        ));
    }
}
