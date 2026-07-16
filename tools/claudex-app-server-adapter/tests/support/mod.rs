use std::{fs, sync::Arc, time::Duration};

use claudex_app_server_adapter::{anthropic::Bridge, app_server::AppServer, http_router};
use reqwest::Client;
use serde_json::{Value, json};
use tempfile::TempDir;

pub struct Adapter {
    server: tokio::task::JoinHandle<()>,
    _home: TempDir,
    pub base_url: String,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Adapter {
    pub async fn start() -> Self {
        Self::start_with_models(None, None).await
    }

    pub async fn start_authenticated(token: &str) -> Self {
        Self::start_configured(None, None, Some(token.to_owned())).await
    }

    pub async fn start_with_models(
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Self {
        Self::start_configured(advisor_model, collaborator_model, None).await
    }

    async fn start_configured(
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        auth_token: Option<String>,
    ) -> Self {
        let home = fixture_home();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind adapter test listener");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("adapter listener address")
        );
        let app_server = AppServer::spawn_with_program(
            "test-main-model",
            env!("CARGO_BIN_EXE_codex-mock"),
            &home.path().join(".codex"),
            &home.path().join("isolated-codex-home"),
        )
        .await
        .expect("start mock app-server");
        assert!(app_server.request("force/error", json!({})).await.is_err());
        let bridge = bridge(app_server, &home, advisor_model, collaborator_model);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                http_router(Arc::new(bridge), "test-main-model".into(), auth_token),
            )
            .await
            .expect("serve adapter test HTTP");
        });
        let adapter = Self {
            server,
            _home: home,
            base_url,
        };
        adapter.wait_until_ready().await;
        adapter
    }

    async fn wait_until_ready(&self) {
        let client = Client::new();
        for _ in 0..100 {
            let ready = client
                .get(format!("{}/health", self.base_url))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            if ready {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("adapter did not become ready");
    }
}

fn fixture_home() -> TempDir {
    let home = tempfile::tempdir().expect("create temporary home");
    fs::create_dir(home.path().join(".codex")).expect("create source CODEX_HOME");
    fs::write(
        home.path().join(".codex/auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test"}}"#,
    )
    .expect("write mock auth");
    fs::create_dir(home.path().join(".claude")).expect("create Claude settings directory");
    fs::write(
        home.path().join(".claude/settings.json"),
        r#"{"effortLevel":"medium"}"#,
    )
    .expect("write Claude settings");
    home
}

fn bridge(
    app_server: Arc<AppServer>,
    home: &TempDir,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> Bridge {
    let bridge = if advisor_model.is_some() || collaborator_model.is_some() {
        Bridge::new_with_subscription_program_and_models(
            app_server,
            "test-main-model".to_owned(),
            env!("CARGO_BIN_EXE_claude-mock"),
            advisor_model.map(str::to_owned),
            collaborator_model.map(str::to_owned),
        )
    } else {
        Bridge::new_with_subscription_program(
            app_server,
            "test-main-model".to_owned(),
            env!("CARGO_BIN_EXE_claude-mock"),
        )
    };
    bridge.with_settings_path(home.path().join(".claude/settings.json"))
}

pub fn base_request() -> Value {
    json!({
        "model":"test-main-model",
        "max_tokens":256,
        "stream":false,
        "system":"Test system prompt",
        "messages":[{"role":"user","content":"Say OK"}]
    })
}

pub async fn post_json(client: &Client, url: &str, body: Value) -> Value {
    client
        .post(url)
        .json(&body)
        .send()
        .await
        .expect("send JSON request")
        .error_for_status()
        .expect("successful JSON status")
        .json()
        .await
        .expect("decode JSON response")
}
