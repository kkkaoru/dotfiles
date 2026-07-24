#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};

    use reqwest::Client;

    use super::*;
    use crate::app_server::AppServer;

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

    #[test]
    fn validates_cli_shape_and_limits() {
        let failures = [
            (vec!["ensure", "--model", "m", "--"], "unexpected arguments"),
            (
                vec!["ensure", "--model", "m", "--inherit-claude-model"],
                "valid only for launch",
            ),
            (
                vec!["serve", "--model", "m", "--inherit-claude-model"],
                "valid only for launch",
            ),
            (vec!["launch", "--model", "m"], "requires `--`"),
            (vec!["serve", "--unknown"], "unknown adapter option"),
            (
                vec!["serve", "--model", "m", "--backend-route", "invalid"],
                "MODEL=BACKEND",
            ),
            (
                vec!["serve", "--model", "m", "--backend-route-json", "invalid"],
                "invalid backend route JSON",
            ),
            (
                vec!["serve", "--provider-config", "/definitely/missing/providers.json"],
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
            (
                vec!["serve", "--model", "m", "--backend-route", "other=grok-acp"],
                "main --model",
            ),
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
    fn parses_valid_cli_options_and_commands() {
        let serve = parse_command(
            [
                "serve",
                "--model",
                "grok-4.5",
                "--backend-route",
                "grok-4.5=grok-acp",
                "--listen",
                "127.0.0.1:9000",
                "--subscription-max-processes",
                "3",
                "--subscription-timeout-minutes",
                "4",
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
        assert!(matches!(launch, RuntimeCommand::Launch(_, _, true)));
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
    }

    #[test]
    fn expands_provider_config_and_internal_route_json() {
        let root = tempfile::tempdir().expect("provider config fixture");
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            r#"{"version":1,"mainProvider":"vendor","providers":[{"id":"vendor","agent":"worker","defaultModel":"vendor-default","effort":"high","modelPrefixes":["vendor-"],"backend":"configured-acp","acp":{"program":"vendor","arguments":["--model","{model}"]}}],"fallback":{"agent":"fallback","model":"sonnet","effort":"high"},"advisor":{"agent":"advisor","model":"fable","effort":"xhigh"}}"#,
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

        let route = serde_json::to_string(&options.routes[0]).unwrap();
        let command = parse_command(
            ["serve", "--model", "vendor-default", "--backend-route-json", &route]
                .into_iter()
                .map(OsString::from)
                .collect(),
        )
        .expect("serialized daemon route");
        assert!(matches!(command, RuntimeCommand::Serve(_)));
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
        let options = AdapterOptions {
            routes: vec![BackendRoute::new("model", BackendKind::CodexAppServer)],
            model: "model".to_owned(),
            listen,
            subscription_max_processes: 2,
            subscription_timeout_minutes: 3,
        };
        let backend = AgentBackend::codex(app_server);
        let server = tokio::spawn(serve_on_listener(options, None, backend, listener));
        let health = Client::new()
            .get(format!("http://{listen}/health"))
            .send()
            .await
            .expect("health response");
        assert!(health.status().is_success());
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
        };
        assert!(serve(options).await.is_err());
    }

    fn script(root: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        path
    }
}
