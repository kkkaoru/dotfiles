use std::ffi::OsString;

use serde_json::json;

use crate::agent_backend::{BackendKind, BackendRoute, WebSearchMode};

use super::{apply, expand_home_with};

#[test]
fn expands_home_only_for_tilde_relative_paths() {
    assert_eq!(
        expand_home_with("catalog.json", Some(OsString::from("/tmp/home"))),
        "catalog.json"
    );
    assert_eq!(
        expand_home_with("~/.codex/catalog.json", Some(OsString::from("/tmp/home"))),
        "/tmp/home/.codex/catalog.json"
    );
    assert_eq!(
        expand_home_with("~/.codex/catalog.json", None),
        "~/.codex/catalog.json"
    );
}

#[test]
fn applies_provider_catalog_and_native_search_configuration() {
    let mut route = BackendRoute::new("gpt", BackendKind::CodexAppServer);
    route.model_provider = Some("openai".to_owned());
    route.model_catalog_json = Some("~/.codex/catalog.json".to_owned());
    route.web_search_mode = WebSearchMode::CodexNative;
    let mut params = json!({"model":"gpt"});

    apply(&route, &mut params);

    assert_eq!(params["modelProvider"], "openai");
    let expected_catalog = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".codex/catalog.json"))
        .expect("coverage test requires HOME");
    assert_eq!(
        params["config"]["model_catalog_json"],
        expected_catalog.to_string_lossy().as_ref()
    );
    assert_eq!(params["config"]["web_search"], "live");
    assert_eq!(params["config"]["features"]["web_search"], true);
}

#[test]
fn leaves_default_route_parameters_unchanged() {
    let route = BackendRoute::new("gpt", BackendKind::CodexAppServer);
    let mut params = json!({"model":"gpt"});

    apply(&route, &mut params);

    assert_eq!(params, json!({"model":"gpt"}));
}

#[test]
fn updates_existing_configuration_for_native_search() {
    let mut route = BackendRoute::new("gpt", BackendKind::CodexAppServer);
    route.web_search_mode = WebSearchMode::CodexNative;
    let mut params = json!({
        "model": "gpt",
        "config": {
            "features": {"unrelated": true},
            "web_search": "disabled"
        }
    });

    apply(&route, &mut params);

    assert_eq!(params["config"]["features"]["unrelated"], true);
    assert_eq!(params["config"]["features"]["web_search"], true);
    assert_eq!(params["config"]["web_search"], "live");
}

#[test]
fn reports_invalid_parameters_before_mutating_route_configuration() {
    let mut route = BackendRoute::new("gpt", BackendKind::CodexAppServer);
    route.model_catalog_json = Some("catalog.json".to_owned());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        apply(&route, &mut json!("invalid"));
    }));

    assert!(result.is_err());
}
