#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::launcher::RetainedGeneration;

    fn proxy_with_pid(pid: u32) -> RetainedProxy {
        RetainedProxy::from_path(
            std::path::PathBuf::from("/nonexistent/retained.json"),
            RetainedGeneration {
                listen: "127.0.0.1:9".parse().unwrap(),
                pid,
                build_id: "old".to_owned(),
                session_ids: vec!["session-a".to_owned()],
                agent_ids: vec!["agent-a".to_owned()],
                agent_ages: std::collections::BTreeMap::new(),
            },
        )
    }

    /// Poison a `RwLock` by panicking while a write guard is held, then
    /// unwinding through `catch_unwind` in the same thread. `std`'s poison
    /// flag is set on guard drop when `thread::panicking()` is true, which
    /// holds during that unwind even though it never escapes this frame.
    fn poison<T>(lock: &std::sync::RwLock<T>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock
                .write()
                .expect("lock must be writable before poisoning");
            panic!("poison for test");
        }));
        assert!(result.is_err(), "poisoning panic must have unwound");
        assert!(lock.write().is_err(), "lock must now report poisoned");
    }

    #[test]
    fn replace_grace_memory_skips_a_poisoned_recent_agents_lock() {
        let proxy = proxy_with_pid(1);
        poison(&proxy.recent_agents);
        // Must not panic even though the write lock it needs is poisoned.
        proxy.replace_grace_memory_for_generation(&["agent-b".to_owned()], &Default::default());
    }

    #[test]
    fn clear_grace_memory_skips_a_poisoned_last_work_at_lock() {
        let proxy = proxy_with_pid(1);
        proxy.mark_recent_work_for_test();
        poison(&proxy.last_work_at);
        proxy.clear_grace_memory();
        // `last_work_at` stayed poisoned, but the still-writable
        // `recent_agents` lock must still be cleared.
        assert!(
            proxy
                .recent_agents
                .read()
                .expect("recent_agents unaffected")
                .is_empty()
        );
    }

    #[test]
    fn clear_grace_memory_skips_a_poisoned_recent_agents_lock() {
        let proxy = proxy_with_pid(1);
        proxy.remember_agent_for_test("agent-b");
        poison(&proxy.recent_agents);
        proxy.clear_grace_memory();
        assert!(
            proxy
                .last_work_at
                .read()
                .expect("last_work_at unaffected")
                .is_none()
        );
    }

    #[test]
    fn clear_session_memory_skips_a_poisoned_sessions_lock() {
        let proxy = proxy_with_pid(1);
        poison(&proxy.sessions);
        // Must not panic; grace memory still clears normally.
        proxy.clear_session_memory();
        assert!(
            proxy
                .recent_agents
                .read()
                .expect("recent_agents unaffected")
                .is_empty()
        );
    }

    #[test]
    fn clear_all_sessions_skips_a_poisoned_sessions_lock_and_does_not_terminate() {
        let proxy = proxy_with_pid(0);
        poison(&proxy.sessions);
        // `pid` reads as 0 (default) because the proxy was built with pid 0,
        // so the terminate branch must be skipped regardless of the lock.
        proxy.clear_all_sessions();
    }

    #[test]
    fn clear_all_sessions_does_not_terminate_when_pid_is_zero() {
        let proxy = proxy_with_pid(0);
        assert!(proxy.owns_cached("session-a"));
        proxy.clear_all_sessions();
        assert!(!proxy.owns_cached("session-a"));
    }
}
