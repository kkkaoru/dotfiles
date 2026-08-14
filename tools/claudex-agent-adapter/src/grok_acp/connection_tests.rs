use super::*;
use std::{
    env,
    ffi::{OsStr, OsString},
    sync::atomic::Ordering,
};
use tokio::process::Command;

#[test]
fn marks_provider_dead_before_notifying_the_driver() {
    let alive = AtomicBool::new(true);
    let (stopped, mut stopped_rx) = oneshot::channel();
    mark_io_stopped(&alive, stopped);
    assert!(!alive.load(Ordering::Relaxed));
    assert_eq!(stopped_rx.try_recv(), Ok(()));
}

#[test]
fn injects_opencode_runtime_config_unless_already_set() {
    let program = OsString::from("opencode");
    let previous = env::var_os("OPENCODE_CONFIG_CONTENT");
    // SAFETY: test restores the process-wide override before returning.
    unsafe {
        env::remove_var("OPENCODE_CONFIG_CONTENT");
    }
    let mut command = Command::new("opencode");
    apply_opencode_acp_runtime_config(&mut command, &program);
    let injected = command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == OsStr::new("OPENCODE_CONFIG_CONTENT"))
        .and_then(|(_, value)| value.map(|value| value.to_owned()));
    assert_eq!(
        injected.as_deref(),
        Some(OsStr::new(OPENCODE_ACP_RUNTIME_CONFIG))
    );

    // SAFETY: test restores the process-wide override before returning.
    unsafe {
        env::set_var("OPENCODE_CONFIG_CONTENT", "user-override");
    }
    let mut command = Command::new("opencode");
    apply_opencode_acp_runtime_config(&mut command, &program);
    let preserved = command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == OsStr::new("OPENCODE_CONFIG_CONTENT"));
    assert!(
        preserved.is_none(),
        "explicit OPENCODE_CONFIG_CONTENT must not be overwritten"
    );

    // SAFETY: restore prior environment for other tests.
    unsafe {
        match previous {
            Some(value) => env::set_var("OPENCODE_CONFIG_CONTENT", value),
            None => env::remove_var("OPENCODE_CONFIG_CONTENT"),
        }
    }

    let mut skipped = Command::new("claude");
    apply_opencode_acp_runtime_config(&mut skipped, &OsString::from("claude"));
    assert!(
        skipped
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("OPENCODE_CONFIG_CONTENT"))
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn start_cleans_up_when_the_provider_exits_during_initialize() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let root = tempfile::tempdir().unwrap();
            let opencode = root.path().join("opencode");
            std::os::unix::fs::symlink("/bin/true", &opencode).unwrap();
            let program = OsString::from(opencode);
            let arguments: Vec<String> = Vec::new();
            let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

            let result = start(StartConnection {
                provider: AcpProvider::Configured,
                program: &program,
                arguments: Some(&arguments),
                model: "test-model",
                effort: None,
                cwd: root.path(),
                events: std::sync::Arc::new(
                    crate::app_server::events::ThreadEventDispatcher::default(),
                ),
                alive,
            })
            .await;

            assert!(result.is_err(), "an exited provider cannot initialize");
        })
        .await;
}
