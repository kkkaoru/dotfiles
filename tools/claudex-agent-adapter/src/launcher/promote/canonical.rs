use std::net::SocketAddr;

use anyhow::{Context, Result, bail};

use super::super::{
    ServiceConfig, daemon_process, daemon_start, fallback, health::Health, live, pending_hot_swap,
};
use super::rebind::{
    listen_is_free, request_bind_listen, request_ephemeral_rebind, restore_old_canonical,
    wait_until_canonical_released,
};
use super::{
    advertised_listen, canonical_serves_current_build, retained_session_ids,
    wait_until_current_build, warm_agent_ages,
};

pub(crate) async fn try_canonical(
    client: &reqwest::Client,
    config: &ServiceConfig,
    health: &Health,
) -> Result<Option<String>> {
    let Some(old_pid) = health.pid.filter(|&pid| pid != 0) else {
        return Ok(None);
    };
    if !health.listener_handover {
        return Ok(None);
    }
    pending_hot_swap::disarm(config);
    let session_ids = retained_session_ids(health);
    let agent_ages = warm_agent_ages(health);
    let advertised = advertised_listen(config, health);
    let warm_listen = fallback::reserve_loopback_listen(config.options.listen)?;
    let warm = config.with_listen(warm_listen);
    let retained_path = live::write_retained_with_agents(
        config,
        advertised,
        old_pid,
        &health.build_id,
        session_ids.clone(),
        agent_ages.clone(),
    )?;
    let started = daemon_start::StartedDaemon::new(
        daemon_start::start_adapter_with_retained(&warm, &retained_path, config)
            .context("warm-start current-build listener before canonical cutover")?,
    );
    if !wait_until_current_build(client, &warm, Some(started.pid())).await {
        bail!(
            "wait for warm-start listener; see {}",
            warm.log_path.display()
        );
    }
    let Some(rebind) = request_ephemeral_rebind(client, config).await? else {
        return Ok(None);
    };
    let retained_listen = live::parse_listen_url(&format!("http://{}", rebind.listen))?;
    live::write_retained_with_agents(
        config,
        retained_listen,
        old_pid,
        &health.build_id,
        session_ids.clone(),
        agent_ages,
    )?;
    wait_until_canonical_released(config).await?;
    let probe = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .context("build live-update probe client")?;
    let _ = request_bind_listen(&probe, &warm, config.options.listen).await;
    if !wait_until_current_build(&probe, config, None).await {
        restore_old_canonical(&probe, config, retained_listen).await;
    }
    if canonical_serves_current_build(&probe, config, None).await {
        return finish_started_promotion(
            started,
            config,
            old_pid,
            retained_listen,
            session_ids.len(),
        );
    }
    drop(started);
    if listen_is_free(config.options.listen) {
        let restarted = daemon_start::StartedDaemon::new(
            daemon_start::start_adapter(config)
                .context("start current-build listener after empty canonical port")?,
        );
        if wait_until_current_build(&probe, config, None).await {
            return finish_started_promotion(
                restarted,
                config,
                old_pid,
                retained_listen,
                session_ids.len(),
            );
        }
    }
    bail!(
        "wait for promoted canonical listener; see {}",
        config.log_path.display()
    );
}

fn finish_started_promotion<T: FnOnce(u32)>(
    started: daemon_start::StartedDaemon<T>,
    config: &ServiceConfig,
    old_pid: u32,
    retained_listen: SocketAddr,
    retained_sessions: usize,
) -> Result<Option<String>> {
    let url = publish_started(started, |pid| {
        publish_promoted(config, pid, old_pid, retained_listen, retained_sessions)
    })?;
    Ok(Some(url))
}

fn publish_started<T, Publish>(
    started: daemon_start::StartedDaemon<T>,
    publish: Publish,
) -> Result<String>
where
    T: FnOnce(u32),
    Publish: FnOnce(u32) -> Result<String>,
{
    let url = publish(started.pid())?;
    started.disarm();
    Ok(url)
}

pub(crate) fn publish_promoted(
    config: &ServiceConfig,
    pid: u32,
    old_pid: u32,
    retained_listen: SocketAddr,
    retained_sessions: usize,
) -> Result<String> {
    live::publish_listen(config, config.options.listen, Some(pid))
        .context("publish promoted adapter ownership")?;
    let _ = live::publish_canonical_rebind(config, config.options.listen, pid);
    if retained_sessions == 0 {
        release_previous(config, old_pid);
        // Cutover wrote retained.json before we knew the generation was idle.
        // Drop the empty snapshot so sticky middleware does not keep probing a
        // released ephemeral listen after reboot / zero-session promotes.
        if let Some((path, _)) = live::load_retained(config) {
            let _ = live::clear_retained(&path);
        }
        eprintln!(
            "claudex: promoted build {} to {} (previous pid {old_pid} released; launch TUI kept)",
            env!("CLAUDEX_BUILD_ID"),
            config.base_url(),
        );
    } else {
        eprintln!(
            "claudex: promoted build {} to {} (previous pid {old_pid} retained on {} for {retained_sessions} in-flight session(s); launch TUI kept)",
            env!("CLAUDEX_BUILD_ID"),
            config.base_url(),
            retained_listen
        );
    }
    Ok(config.base_url())
}

pub(crate) fn release_previous(config: &ServiceConfig, old_pid: u32) {
    if daemon_process::matches(old_pid, &config.executable) {
        daemon_process::terminate(old_pid);
    }
}

#[cfg(test)]
mod ownership_tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn failed_publication_keeps_the_new_daemon_guard_armed() {
        let terminated = Cell::new(0);
        let started = daemon_start::StartedDaemon::with_terminate(91, |pid| terminated.set(pid));

        let error = publish_started(started, |_| anyhow::bail!("publish failed"))
            .expect_err("publication failure must be returned");

        assert_eq!(error.to_string(), "publish failed");
        assert_eq!(terminated.get(), 91);
    }

    #[test]
    fn successful_publication_transfers_daemon_ownership() {
        let terminated = Cell::new(0);
        let started = daemon_start::StartedDaemon::with_terminate(92, |pid| terminated.set(pid));

        let url = publish_started(started, |pid| {
            assert_eq!(pid, 92);
            Ok("http://127.0.0.1:8318".to_owned())
        })
        .expect("publication succeeds");

        assert_eq!(url, "http://127.0.0.1:8318");
        assert_eq!(terminated.get(), 0);
    }
}
