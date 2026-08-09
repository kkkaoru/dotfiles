use tracing_subscriber::EnvFilter;

const LOG_FORMAT_ENV: &str = "CLAUDEX_LOG_FORMAT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LogFormat {
    Json,
    Compact,
}

pub(crate) fn configured_format(value: Option<&str>) -> LogFormat {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("compact") | Some("text") => LogFormat::Compact,
        _ => LogFormat::Json,
    }
}

pub(crate) fn init() {
    let format = configured_format(std::env::var(LOG_FORMAT_ENV).ok().as_deref());
    match format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .flatten_event(true)
                .with_current_span(true)
                .with_span_list(true)
                .with_env_filter(default_filter())
                .with_writer(std::io::stderr)
                .try_init()
                .ok();
        }
        LogFormat::Compact => {
            tracing_subscriber::fmt()
                .with_env_filter(default_filter())
                .with_writer(std::io::stderr)
                .try_init()
                .ok();
        }
    }
}

fn default_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_json_is_the_default_log_format() {
        assert_eq!(configured_format(None), LogFormat::Json);
        assert_eq!(configured_format(Some("json")), LogFormat::Json);
        assert_eq!(configured_format(Some("unknown")), LogFormat::Json);
    }

    #[test]
    fn compact_text_is_an_explicit_compatibility_opt_out() {
        assert_eq!(configured_format(Some("compact")), LogFormat::Compact);
        assert_eq!(configured_format(Some(" TEXT ")), LogFormat::Compact);
    }

    #[test]
    fn init_is_idempotent_for_json_and_compact() {
        let previous_format = std::env::var_os(LOG_FORMAT_ENV);
        let previous_filter = std::env::var_os("RUST_LOG");
        unsafe {
            std::env::set_var(LOG_FORMAT_ENV, "compact");
            std::env::set_var("RUST_LOG", "warn");
        }
        init();
        unsafe { std::env::set_var(LOG_FORMAT_ENV, "json") };
        init();
        unsafe { std::env::remove_var("RUST_LOG") };
        init();
        match previous_format {
            Some(value) => unsafe { std::env::set_var(LOG_FORMAT_ENV, value) },
            None => unsafe { std::env::remove_var(LOG_FORMAT_ENV) },
        }
        match previous_filter {
            Some(value) => unsafe { std::env::set_var("RUST_LOG", value) },
            None => unsafe { std::env::remove_var("RUST_LOG") },
        }
    }
}
