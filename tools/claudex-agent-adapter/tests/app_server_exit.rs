use std::fs;

use claudex_agent_adapter::app_server::AppServer;
use serde_json::json;

#[tokio::test]
async fn reports_app_server_exit_to_pending_request() {
    let home = tempfile::tempdir().expect("create temporary home");
    fs::create_dir(home.path().join("source")).unwrap();
    fs::write(home.path().join("source/auth.json"), "{}").unwrap();
    let app = AppServer::spawn_with_program(
        "test-main-model",
        env!("CARGO_BIN_EXE_codex-mock"),
        &home.path().join("source"),
        &home.path().join("isolated"),
    )
    .await
    .unwrap();
    let error = app.request("force/exit", json!({})).await.unwrap_err();
    assert!(error.to_string().contains("app-server exited"));
    assert!(!app.is_alive());
    assert!(app.request("after/exit", json!({})).await.is_err());
}
