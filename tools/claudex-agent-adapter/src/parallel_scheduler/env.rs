pub(crate) fn parse_usize_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

pub(crate) fn parse_u64_env(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

pub(crate) fn parse_bool_env(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| match value.as_str() {
            "1" | "true" | "TRUE" | "True" | "yes" | "YES" | "on" | "ON" => Some(true),
            "0" | "false" | "FALSE" | "False" | "no" | "NO" | "off" | "OFF" => Some(false),
            _ => None,
        })
}
