use super::*;
use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use std::sync::Arc;

const LOG_CAPTURE_CHILD_ENV: &str = "CLAUDEX_SESSION_SCOPE_LOG_CAPTURE_CHILD";
const CREATE_LOG_TEST: &str =
    "agent_backend::session_scope::tests::scope_create_and_reuse_emit_structured_log_events";
const RELEASE_LOG_TEST: &str =
    "agent_backend::session_scope::tests::scope_release_emits_structured_log_event";

#[test]
fn scope_key_uses_anonymous_for_missing_ids() {
    assert_eq!(
        SessionScopedBackends::scope_key(None),
        ANONYMOUS_SESSION_SCOPE
    );
    assert_eq!(
        SessionScopedBackends::scope_key(Some("")),
        ANONYMOUS_SESSION_SCOPE
    );
    assert_eq!(SessionScopedBackends::scope_key(Some("sess-a")), "sess-a");
}

#[test]
fn distinct_claude_sessions_get_independent_routed_pools() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let a = scopes.scope(Some("session-a"));
    let b = scopes.scope(Some("session-b"));
    assert!(!Arc::ptr_eq(&a, &b));
    assert_eq!(scopes.scope_count(), 2);
    assert!(Arc::ptr_eq(&a, &scopes.scope(Some("session-a"))));
}

#[test]
fn concurrent_scope_lookup_reuses_one_pool_per_session_without_crossing_sessions() {
    let scopes = Arc::new(SessionScopedBackends::new(&[BackendRoute::new(
        "main",
        BackendKind::CodexAppServer,
    )]));
    let mut workers = Vec::new();
    for index in 0..32 {
        workers.push(spawn_scope_lookup(&scopes, index));
    }
    let addresses = workers
        .into_iter()
        .map(|worker| worker.join().expect("scope worker"))
        .collect::<Vec<_>>();
    let a = scopes.scope(Some("parallel-a"));
    let b = scopes.scope(Some("parallel-b"));
    assert_eq!(scopes.scope_count(), 2);
    assert!(
        addresses
            .iter()
            .enumerate()
            .all(|(index, address)| *address == expected_scope_ptr(index, &a, &b))
    );
    assert!(!Arc::ptr_eq(&a, &b));
}

fn spawn_scope_lookup(
    scopes: &Arc<SessionScopedBackends>,
    index: usize,
) -> std::thread::JoinHandle<usize> {
    let scopes = Arc::clone(scopes);
    std::thread::spawn(move || parallel_scope_address(&scopes, index))
}

/// The `parallel-a`/`parallel-b` pool address a concurrent lookup should observe.
fn parallel_scope_address(scopes: &SessionScopedBackends, index: usize) -> usize {
    let id = if index.is_multiple_of(2) {
        "parallel-a"
    } else {
        "parallel-b"
    };
    Arc::as_ptr(&scopes.scope(Some(id))) as usize
}

fn expected_scope_ptr(index: usize, a: &Arc<AgentBackend>, b: &Arc<AgentBackend>) -> usize {
    let target = if index.is_multiple_of(2) { a } else { b };
    Arc::as_ptr(target) as usize
}

#[tokio::test]
async fn release_scope_drops_the_pool() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let _ = scopes.scope(Some("session-a"));
    assert_eq!(scopes.scope_count(), 1);
    scopes.release_scope(Some("session-a")).await;
    assert_eq!(scopes.scope_count(), 0);
}

#[tokio::test]
async fn release_scope_waits_for_leaf_shutdown_before_scope_is_gone() {
    let scopes = SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::GrokAcp)]);
    let leaf = Arc::new(AgentBackend::Grok(
        crate::grok_acp::GrokAcp::alive_for_test(),
    ));
    scopes.insert_scope_for_test(
        "shutdown-order",
        AgentBackend::routed(vec![("main".to_owned(), Arc::clone(&leaf))]),
    );

    scopes.release_scope(Some("shutdown-order")).await;

    assert!(!leaf.is_alive(), "scope release must await leaf cleanup");
    assert_eq!(scopes.scope_count(), 0);
}

#[test]
fn empty_scopes_report_models_alive_and_catalog_metadata() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    assert!(scopes.model_is_alive("main"));
    assert!(scopes.started_models().is_empty());
    assert!(scopes.catalog().supports("main"));
}

#[tokio::test]
async fn shutdown_all_clears_every_scope() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let _ = scopes.scope(Some("a"));
    let _ = scopes.scope(Some("b"));
    assert_eq!(scopes.scope_count(), 2);
    scopes.shutdown_all().await;
    assert_eq!(scopes.scope_count(), 0);
}

#[test]
fn scope_snapshots_sort_and_report_started_models() {
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let _ = scopes.scope(Some("sess-b"));
    let _ = scopes.scope(Some("sess-a"));
    let snapshots = scopes.scope_snapshots();
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.claude_session_id.as_str())
            .collect::<Vec<_>>(),
        ["sess-a", "sess-b"]
    );
    assert!(
        snapshots
            .iter()
            .all(|snapshot| snapshot.started_models.is_empty())
    );
}

#[test]
fn unique_started_pool_is_none_for_lazy_scopes() {
    let scopes = SessionScopedBackends::new(&[BackendRoute::new(
        "glm-5.2:cloud",
        BackendKind::CodexAppServer,
    )]);
    let _ = scopes.scope(Some("tui-session"));
    assert!(
        scopes
            .unique_started_pool_for_model("glm-5.2:cloud")
            .is_none()
    );
}

#[test]
fn scope_or_self_clones_non_scoped_backends() {
    let leaf = AgentBackend::spawn_routes(&[]);
    let scoped = leaf.scope_or_self(Some("sess"));
    assert!(!Arc::ptr_eq(&leaf, &scoped));
    let codex = AgentBackend::routed(Vec::new());
    assert!(Arc::ptr_eq(&codex, &codex.scope_or_self(Some("sess"))));
}

#[test]
fn scope_create_and_reuse_emit_structured_log_events() {
    if isolated_log_capture("create") {
        assert_scope_create_and_reuse_logs();
        return;
    }
    run_isolated_log_capture(CREATE_LOG_TEST, "create");
}

/// Capture this event assertion in a fresh test process: a concurrent test may
/// install a global tracing subscriber whose cached callsite interest excludes
/// the thread-local capture subscriber used below.
fn assert_scope_create_and_reuse_logs() {
    use tracing::level_filters::LevelFilter;

    let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let _guard = capture_tracing_logs(&buffer, LevelFilter::DEBUG);
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let _ = scopes.scope(Some("log-sess"));
    let _ = scopes.scope(Some("log-sess"));
    let text = String::from_utf8(buffer.lock().expect("log buffer").clone()).unwrap();
    assert!(
        text.contains("provider_session_scope_create"),
        "missing create event in logs: {text}"
    );
    assert!(
        text.contains("provider_session_scope_reuse"),
        "missing reuse event in logs: {text}"
    );
    assert!(
        text.contains("log-sess"),
        "missing session id in logs: {text}"
    );
}

#[tokio::test]
async fn scope_release_emits_structured_log_event() {
    match isolated_log_capture("release") {
        true => assert_scope_release_logs().await,
        false => run_isolated_log_capture(RELEASE_LOG_TEST, "release"),
    }
}

async fn assert_scope_release_logs() {
    use tracing::level_filters::LevelFilter;

    let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let _guard = capture_tracing_logs(&buffer, LevelFilter::INFO);
    let scopes =
        SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
    let _ = scopes.scope(Some("release-sess"));
    scopes.release_scope(Some("release-sess")).await;
    let text = String::from_utf8(buffer.lock().expect("log buffer").clone()).unwrap();
    assert!(
        text.contains("provider_session_scope_release"),
        "missing release event in logs: {text}"
    );
    assert!(
        text.contains("release-sess"),
        "missing session id in logs: {text}"
    );
}

fn isolated_log_capture(kind: &str) -> bool {
    std::env::var(LOG_CAPTURE_CHILD_ENV).is_ok_and(|child| child == kind)
}

fn run_isolated_log_capture(test_name: &str, kind: &str) {
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .arg(test_name)
        .arg("--exact")
        .arg("--test-threads=1")
        .env(LOG_CAPTURE_CHILD_ENV, kind)
        .output()
        .expect("run isolated tracing capture test");
    assert!(
        output.status.success(),
        "isolated tracing capture test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BufferWriter(Arc::clone(&self.0))
    }
}

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Installs a scoped tracing subscriber and refreshes tracing's global callsite
/// interest cache both when the scope starts and after it is removed.
struct TracingCaptureGuard {
    default_guard: Option<tracing::subscriber::DefaultGuard>,
}

impl Drop for TracingCaptureGuard {
    fn drop(&mut self) {
        drop(self.default_guard.take());
        tracing::callsite::rebuild_interest_cache();
    }
}

/// Install a tracing subscriber that appends formatted log lines into `buffer`,
/// returning the guard that must stay alive for the duration of the test.
fn capture_tracing_logs(
    buffer: &Arc<std::sync::Mutex<Vec<u8>>>,
    max_level: tracing::level_filters::LevelFilter,
) -> TracingCaptureGuard {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(max_level)
        .with_writer(BufferWriter(Arc::clone(buffer)))
        .with_ansi(false)
        .finish();
    let default_guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    TracingCaptureGuard {
        default_guard: Some(default_guard),
    }
}
