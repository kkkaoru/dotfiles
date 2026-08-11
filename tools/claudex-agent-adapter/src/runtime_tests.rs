// Coverage gates measure production code; test implementations are excluded.
#![cfg_attr(coverage_nightly, coverage(off))]

use std::{ffi::OsString, os::unix::fs::PermissionsExt, path::PathBuf};

use reqwest::Client;

use super::*;
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::app_server::AppServer;

async fn wait_for_health(client: &Client, url: &str) -> reqwest::Response {
    let attempts = if cfg!(coverage_nightly) { 80 } else { 30 };
    for _ in 0..attempts {
        if let Ok(response) = client.get(url).send().await {
            return response;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("health response");
}

#[test]
fn parses_token_helpers() {
    assert_eq!(
        nonempty_token(Some("token".to_owned())).as_deref(),
        Some("token")
    );
    assert_eq!(nonempty_token(Some(String::new())), None);
    assert_eq!(nonempty_token(None), None);
    assert!(utf8(Some("model".into()), "model").is_ok());
    assert!(utf8(None, "model").is_err());
}

fn assert_cli_shape_failures_basic() {
    let failures = [
        (vec!["ensure", "--model", "m", "--"], "unexpected arguments"),
        (
            vec!["ensure", "--model", "m", "--inherit-claude-model"],
            "valid only for launch",
        ),
        (
            vec!["hot-swap", "--model", "m", "--inherit-claude-model"],
            "valid only for launch",
        ),
        (
            vec!["hot-swap", "--model", "m", "--"],
            "unexpected arguments",
        ),
        (
            vec!["ensure", "--model", "m", "--wait-idle"],
            "unknown adapter option",
        ),
        (
            vec!["serve", "--model", "m", "--wait-idle"],
            "unknown adapter option",
        ),
        (
            vec!["serve", "--model", "m", "--inherit-claude-model"],
            "valid only for launch",
        ),
        (vec!["launch", "--model", "m"], "requires `--`"),
        (vec!["serve", "--unknown"], "unknown adapter option"),
    ];
    for (arguments, expected) in failures {
        let arguments = arguments.into_iter().map(OsString::from).collect();
        assert!(
            parse_command(arguments)
                .expect_err("invalid CLI must fail")
                .to_string()
                .contains(expected)
        );
    }
}

fn assert_cli_shape_failures_routes_and_limits() {
    let failures = [
        (
            vec!["serve", "--model", "m", "--backend-route", "invalid"],
            "MODEL=BACKEND",
        ),
        (
            vec!["serve", "--model", "m", "--backend-route-json", "invalid"],
            "invalid backend route JSON",
        ),
        (
            vec!["serve", "--model", "m", "--worker-route-json", "invalid"],
            "invalid worker route JSON",
        ),
        (
            vec![
                "serve",
                "--provider-config",
                "/definitely/missing/providers.json",
            ],
            "read provider config",
        ),
        (
            vec![
                "serve",
                "--model",
                "m",
                "--backend-route",
                "m=grok-acp",
                "--backend-route",
                "m=codex-app-server",
            ],
            "must be unique",
        ),
        (vec!["serve", "--model", ""], "--model must not be empty"),
        (
            vec!["serve", "--model", "m", "--subscription-max-processes", "0"],
            "positive integer",
        ),
        (
            vec![
                "serve",
                "--model",
                "m",
                "--subscription-timeout-minutes",
                "18446744073709551615",
            ],
            "out of range",
        ),
        (
            vec![
                "serve",
                "--model",
                "m",
                "--subscription-max-processes",
                "18446744073709551615",
            ],
            "out of range",
        ),
    ];
    for (arguments, expected) in failures {
        let arguments = arguments.into_iter().map(OsString::from).collect();
        assert!(
            parse_command(arguments)
                .expect_err("invalid CLI must fail")
                .to_string()
                .contains(expected)
        );
    }
}

#[test]
fn validates_cli_shape_and_limits() {
    assert_cli_shape_failures_basic();
    assert_cli_shape_failures_routes_and_limits();
}

#[test]
fn rejects_invalid_search_worker_route_json() {
    let arguments = [
        "serve",
        "--model",
        "m",
        "--search-worker-route-json",
        "invalid",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    assert!(
        parse_command(arguments)
            .expect_err("invalid search worker route must fail")
            .to_string()
            .contains("invalid search worker route JSON")
    );
}

#[test]
fn rejects_an_invalid_backend_route_concurrency_limit() {
    let arguments = [
        "serve",
        "--model",
        "m",
        "--backend-route-json",
        r#"{"model":"m","backend":"grok-acp","maxConcurrency":0}"#,
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    assert!(
        parse_command(arguments)
            .expect_err("zero route concurrency must fail")
            .to_string()
            .contains("maxConcurrency")
    );
}

fn assert_parses_valid_cli_options_part1() {
    let serve = parse_command(
        [
            "serve",
            "--model",
            "grok-4.5",
            "--backend-route",
            "grok-4.5=grok-acp",
            "--worker-route-json",
            r#"{"agent":"claudex-grok","model":"grok-4.5","effort":"high"}"#,
            "--search-worker-route-json",
            r#"{"agent":"claudex-search","model":"gpt-search","effort":"xhigh"}"#,
            "--listen",
            "127.0.0.1:9000",
            "--subscription-max-processes",
            "3",
            "--subscription-timeout-minutes",
            "4",
            "--subagent-hard-timeout-seconds",
            "17",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
    )
    .expect("valid serve command");
    let RuntimeCommand::Serve(options) = serve else {
        panic!("serve command expected");
    };
    assert_eq!(options.listen, "127.0.0.1:9000".parse().unwrap());
    assert_eq!(options.subscription_max_processes, 3);
    assert_eq!(options.subscription_timeout_minutes, 4);
    assert_eq!(
        options
            .subagent_hard_timeout_seconds
            .map(std::num::NonZeroU64::get),
        Some(17)
    );
    assert_eq!(
        options.model_catalog.worker_fields("claudex-grok"),
        Some(("grok-4.5", "high"))
    );
    assert_eq!(
        options.model_catalog.search_worker_routes(),
        &[crate::provider_config::WorkerRoute::new(
            "claudex-search".to_owned(),
            "gpt-search".to_owned(),
            "xhigh".to_owned()
        )]
    );

    let launch = parse_command(
        [
            "launch",
            "--model",
            "m",
            "--inherit-claude-model",
            "--",
            "--continue",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
    )
    .expect("valid launch command");
    let RuntimeCommand::Launch(_, _, true) = launch else {
        panic!("launch command expected");
    };
}

fn assert_parses_valid_cli_options_part2() {
    assert!(matches!(
        parse_command(
            ["ensure", "--model", "m"]
                .into_iter()
                .map(OsString::from)
                .collect()
        )
        .expect("valid ensure command"),
        RuntimeCommand::Ensure(_)
    ));
    assert!(matches!(
        parse_command(
            ["hot-swap", "--model", "m"]
                .into_iter()
                .map(OsString::from)
                .collect()
        )
        .expect("valid hot-swap command"),
        RuntimeCommand::HotSwap(_, false)
    ));
    assert!(matches!(
        parse_command(
            ["hot-swap", "--wait-idle", "--model", "m"]
                .into_iter()
                .map(OsString::from)
                .collect()
        )
        .expect("hot-swap wait-idle before options"),
        RuntimeCommand::HotSwap(_, true)
    ));
    assert!(matches!(
        parse_command(
            [
                "hot-swap",
                "--model",
                "m",
                "--wait-idle",
                "--listen",
                "127.0.0.1:8318"
            ]
            .into_iter()
            .map(OsString::from)
            .collect()
        )
        .expect("hot-swap wait-idle after options"),
        RuntimeCommand::HotSwap(_, true)
    ));
}

#[test]
fn parses_valid_cli_options_and_commands() {
    assert_parses_valid_cli_options_part1();
    assert_parses_valid_cli_options_part2();
}

#[cfg(unix)]
#[test]
fn parses_provider_defaults_and_rejects_non_utf8_option_names() {
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().expect("provider config fixture");
    let path = root.path().join("providers.json");
    std::fs::write(
        &path,
        r#"{"version":1,"mainProviders":["vendor"],"providers":[{"id":"vendor","agent":"worker","defaultModel":"vendor-default","effort":"high","modelPrefixes":["vendor-"],"backend":"configured-acp","acp":{"program":"vendor","arguments":["--model","{model}"]}}],"fallback":{"agent":"fallback","model":"sonnet","effort":"high"}}"#,
    )
    .expect("provider config");
    let command = parse_command(
        [
            OsString::from("serve"),
            OsString::from("--provider-config"),
            path.clone().into_os_string(),
        ]
        .into_iter()
        .collect(),
    )
    .expect("provider configuration supplies routes without a main model");
    let RuntimeCommand::Serve(options) = command else {
        panic!("serve command expected");
    };
    assert!(options.model.is_empty());
    assert_eq!(
        options.model_catalog.worker_fields("worker"),
        Some(("vendor-default", "high"))
    );

    let launch = parse_command(
        [
            OsString::from("launch"),
            OsString::from("--provider-config"),
            path.into_os_string(),
            OsString::from("--"),
            OsString::from("--resume"),
            OsString::from("session-id"),
        ]
        .into_iter()
        .collect(),
    )
    .expect("provider launch inherits the Claude Code request model");
    let RuntimeCommand::Launch(options, arguments, inherit) = launch else {
        panic!("launch command expected");
    };
    assert!(options.model.is_empty());
    assert!(inherit);
    assert_eq!(arguments, ["--resume", "session-id"]);

    let error = parse_command(
        [
            OsString::from("serve"),
            OsString::from("--model"),
            OsString::from("model"),
            OsString::from_vec(vec![0xff]),
        ]
        .into_iter()
        .collect(),
    )
    .expect_err("non-UTF-8 option must fail");
    assert!(error.to_string().contains("valid UTF-8"));
}

#[test]
fn expands_provider_config_and_internal_route_json() {
    let root = tempfile::tempdir().expect("provider config fixture");
    let path = root.path().join("providers.json");
    std::fs::write(
        &path,
        r#"{"version":1,"mainProviders":["vendor"],"providers":[{"id":"vendor","agent":"worker","defaultModel":"vendor-default","effort":"high","modelPrefixes":["vendor-"],"selectableModels":["vendor-terra"],"backend":"configured-acp","acp":{"program":"vendor","arguments":["--model","{model}"]}}],"fallback":{"agent":"fallback","model":"sonnet","effort":"high"}}"#,
    )
    .expect("provider config");
    let command = parse_command(
        [
            OsString::from("serve"),
            OsString::from("--provider-config"),
            path.into_os_string(),
            OsString::from("--model"),
            OsString::from("vendor-next"),
        ]
        .into_iter()
        .collect(),
    )
    .expect("config-driven command");
    let RuntimeCommand::Serve(options) = command else {
        panic!("serve command expected");
    };
    assert_eq!(options.model, "vendor-next");
    assert_eq!(options.routes[0].backend, BackendKind::ConfiguredAcp);
    assert_eq!(
        options.model_catalog.selectable_models(),
        &["vendor-terra".to_owned()]
    );

    let route = serde_json::to_string(&options.routes[0]).unwrap();
    let command = parse_command(
        [
            "serve",
            "--model",
            "vendor-default",
            "--backend-route-json",
            &route,
            "--selectable-model",
            "vendor-terra",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
    )
    .expect("serialized daemon route");
    let RuntimeCommand::Serve(served) = command else {
        panic!("serve command expected");
    };
    assert_eq!(
        served.model_catalog.selectable_models(),
        &["vendor-terra".to_owned()]
    );
}

#[tokio::test]
async fn runs_the_build_id_command() {
    assert_eq!(
        run(["adapter".into(), "build-id".into()])
            .await
            .expect("build ID command"),
        0
    );
}

#[tokio::test]
async fn serves_a_preconfigured_app_server() {
    let root = tempfile::tempdir().expect("runtime fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth");
    let program = script(
        root.path(),
        "app-server",
        "read line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
    );
    let app_server =
        AppServer::spawn_with_program("model", program, &source, &root.path().join("isolated"))
            .await
            .expect("mock app-server");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let listen = listener.local_addr().expect("listener address");
    let mut options = AdapterOptions {
        routes: vec![BackendRoute::new("model", BackendKind::CodexAppServer)],
        model: "model".to_owned(),
        listen,
        subscription_max_processes: 2,
        subscription_timeout_minutes: 3,
        subagent_hard_timeout_seconds: None,
        model_catalog: crate::provider_config::ModelCatalog::default(),
    };
    options
        .model_catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "worker".to_owned(),
            "model".to_owned(),
            "high".to_owned(),
        )])
        .expect("worker route");
    let backend = AgentBackend::codex(app_server);
    let server = tokio::spawn(serve_on_listener(options, None, backend, listener));
    let url = format!("http://{listen}/health");
    let client = Client::new();
    let health = wait_for_health(&client, &url).await;
    assert!(health.status().is_success());
    let request_id = health
        .headers()
        .get("x-claudex-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("structured request ID response header");
    assert!(
        uuid::Uuid::parse_str(request_id).is_ok(),
        "request ID must be a UUID: {request_id}"
    );
    let health = health
        .json::<serde_json::Value>()
        .await
        .expect("health JSON");
    assert_eq!(
        health["worker_routes"][0],
        r#"{"agent":"worker","model":"model","effort":"high"}"#
    );
    assert_eq!(health["active_http_requests"], 0);
    assert_eq!(health["active_provider_turns"], 0);
    assert!(health["subagent_hard_timeout_seconds"].is_null());
    server.abort();
}

#[tokio::test]
async fn rejects_invalid_limits_before_serving() {
    let root = tempfile::tempdir().expect("runtime fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth");
    let program = script(
        root.path(),
        "app-server",
        "read line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
    );
    let app_server =
        AppServer::spawn_with_program("model", program, &source, &root.path().join("isolated"))
            .await
            .expect("mock app-server");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let options = AdapterOptions {
        routes: vec![BackendRoute::new("model", BackendKind::CodexAppServer)],
        model: "model".to_owned(),
        listen: listener.local_addr().expect("listener address"),
        subscription_max_processes: 0,
        subscription_timeout_minutes: 1,
        subagent_hard_timeout_seconds: None,
        model_catalog: crate::provider_config::ModelCatalog::default(),
    };
    assert!(
        serve_on_listener(options, None, AgentBackend::codex(app_server), listener)
            .await
            .is_err()
    );

    let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("occupied listener");
    let options = AdapterOptions {
        routes: vec![BackendRoute::new("model", BackendKind::CodexAppServer)],
        model: "model".to_owned(),
        listen: occupied.local_addr().expect("occupied address"),
        subscription_max_processes: 1,
        subscription_timeout_minutes: 1,
        subagent_hard_timeout_seconds: None,
        model_catalog: crate::provider_config::ModelCatalog::default(),
    };
    assert!(serve(options).await.is_err());
}

fn script(root: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("executable");
    path
}
