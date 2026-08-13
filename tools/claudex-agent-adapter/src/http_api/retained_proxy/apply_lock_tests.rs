fn next_generation(pid: u32) -> RetainedGeneration {
    RetainedGeneration {
        listen: "127.0.0.1:19".parse().unwrap(),
        pid,
        build_id: "next".to_owned(),
        session_ids: vec!["session-b".to_owned()],
        agent_ids: Vec::new(),
        agent_ages: std::collections::BTreeMap::new(),
    }
}

#[test]
fn apply_generation_skips_a_poisoned_listen_lock() {
    let proxy = proxy_with_pid(1);
    poison(&proxy.listen);
    proxy.apply_generation(next_generation(1));
}

#[test]
fn apply_generation_skips_a_poisoned_pid_lock() {
    let proxy = proxy_with_pid(1);
    poison(&proxy.pid);
    proxy.apply_generation(next_generation(2));
}

#[test]
fn apply_generation_skips_a_poisoned_sessions_lock() {
    let proxy = proxy_with_pid(1);
    poison(&proxy.sessions);
    proxy.apply_generation(next_generation(1));
}

#[test]
fn mark_recent_work_skips_a_poisoned_last_work_at_lock() {
    let proxy = proxy_with_pid(1);
    poison(&proxy.last_work_at);
    proxy.mark_recent_work_for_test();
}

#[test]
fn remember_agent_skips_a_poisoned_recent_agents_lock() {
    let proxy = proxy_with_pid(1);
    poison(&proxy.recent_agents);
    proxy.remember_agent_for_test("agent-z");
}
