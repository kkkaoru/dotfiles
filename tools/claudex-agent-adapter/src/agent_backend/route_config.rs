use serde_json::Value;

use super::BackendRoute;

pub(super) fn apply(route: &BackendRoute, params: &mut Value) {
    if let Some(provider) = &route.model_provider {
        params["modelProvider"] = Value::String(provider.clone());
    }
    if let Some(catalog) = &route.model_catalog_json {
        let config = params
            .as_object_mut()
            .expect("thread/start params must be an object")
            .entry("config")
            .or_insert_with(|| serde_json::json!({}));
        config["model_catalog_json"] = Value::String(expand_home(catalog));
    }
    if route.web_search_mode == crate::agent_backend::WebSearchMode::CodexNative {
        let config = params
            .as_object_mut()
            .expect("thread/start params must be an object")
            .entry("config")
            .or_insert_with(|| serde_json::json!({}));
        config["web_search"] = Value::String("live".to_owned());
        config["features"]["web_search"] = Value::Bool(true);
    }
}

fn expand_home(path: &str) -> String {
    expand_home_with(path, std::env::var_os("HOME"))
}

fn expand_home_with(path: &str, home: Option<std::ffi::OsString>) -> String {
    let Some(relative) = path.strip_prefix("~/") else {
        return path.to_owned();
    };
    home.map_or_else(
        || path.to_owned(),
        |home| {
            std::path::PathBuf::from(home)
                .join(relative)
                .to_string_lossy()
                .into_owned()
        },
    )
}

#[cfg(test)]
#[path = "route_config_tests.rs"]
mod tests;
