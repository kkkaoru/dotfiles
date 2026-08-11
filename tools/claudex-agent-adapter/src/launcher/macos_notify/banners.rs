use std::net::SocketAddr;

use super::{Event, ServiceConfig, post};

pub(in crate::launcher) fn waiting_for_idle(config: &ServiceConfig, waiter_pid: u32) {
    let Some(cache) = config.log_path.parent() else {
        return;
    };
    post(
        cache,
        &config.options.listen,
        Event::WaitingForIdle {
            listen: config.options.listen.to_string(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            waiter_pid,
        },
    );
}

pub(in crate::launcher) fn live_ready(config: &ServiceConfig, live_listen: SocketAddr) {
    if live_listen == config.options.listen {
        return;
    }
    let Some(cache) = config.log_path.parent() else {
        return;
    };
    post(
        cache,
        &config.options.listen,
        Event::LiveReady {
            listen: live_listen.to_string(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            waiting: config.options.listen.to_string(),
        },
    );
}

pub(in crate::launcher) fn swap_complete(config: &ServiceConfig) {
    let Some(cache) = config.log_path.parent() else {
        return;
    };
    post(
        cache,
        &config.options.listen,
        Event::SwapComplete {
            listen: config.options.listen.to_string(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
        },
    );
}
