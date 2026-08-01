pub(super) fn sanitize_diagnostic(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let mut redact_remaining = 0;
    let mut pieces = Vec::new();
    for piece in normalized.split_whitespace() {
        let lowercase = piece.to_ascii_lowercase();
        if redact_remaining > 0 {
            pieces.push("[REDACTED]".to_owned());
            redact_remaining -= 1;
            continue;
        }
        if is_sensitive_key(&lowercase) {
            pieces.push(piece.to_owned());
            redact_remaining = 2;
        } else if has_sensitive_value(&lowercase) {
            pieces.push("[REDACTED]".to_owned());
            redact_remaining = usize::from(redacts_following_value(&lowercase));
        } else {
            pieces.push(piece.to_owned());
        }
    }
    truncate(&pieces.join(" "), super::MAX_DIAGNOSTIC_CHARS)
}

fn redacts_following_value(value: &str) -> bool {
    value == "bearer:"
        || value == "bearer="
        || value.starts_with("authorization:")
        || value.starts_with("authorization=")
        || sensitive_keys()
            .any(|key| value.ends_with(&format!("{key}:")) || value.ends_with(&format!("{key}=")))
}

fn is_sensitive_key(value: &str) -> bool {
    let normalized = value
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_');
    value == "bearer" || sensitive_keys().any(|key| key == normalized)
}

fn has_sensitive_value(value: &str) -> bool {
    value.starts_with("sk-")
        || value.starts_with("bearer=")
        || value.starts_with("bearer:")
        || sensitive_keys()
            .any(|key| value.contains(&format!("{key}=")) || value.contains(&format!("{key}:")))
}

fn sensitive_keys() -> impl Iterator<Item = &'static str> + Clone {
    [
        "authorization",
        "api_key",
        "api-key",
        "apikey",
        "api_token",
        "access_token",
        "refresh_token",
        "token",
        "cookie",
    ]
    .into_iter()
}

fn truncate(value: &str, limit: usize) -> String {
    let mut characters = value.chars();
    let truncated = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
mod tests {
    use super::*;

    #[test]
    fn covers_sensitive_key_and_value_boundaries() {
        for key in [
            "bearer",
            "authorization",
            "api_key",
            "api-key",
            "apikey",
            "api_token",
            "access_token",
            "refresh_token",
            "token",
            "cookie",
        ] {
            assert!(is_sensitive_key(key));
            if key != "bearer" {
                assert!(is_sensitive_key(&format!("[{key}]")));
            }
        }
        assert!(!is_sensitive_key("ordinary"));
        assert!(has_sensitive_value("sk-test"));
        assert!(has_sensitive_value("bearer=fixture"));
        assert!(has_sensitive_value("bearer:fixture"));
        assert!(has_sensitive_value("cookie=fixture"));
        assert!(!has_sensitive_value("ordinary"));

        for value in [
            "bearer:",
            "bearer=",
            "authorization:fixture",
            "authorization=fixture",
            "token:",
            "api-key=",
        ] {
            assert!(redacts_following_value(value));
        }
        assert!(!redacts_following_value("ordinary"));
    }

    #[test]
    fn preserves_exact_limit_and_normalizes_control_characters() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("abcdef", 3), "abc...");
        assert_eq!(sanitize_diagnostic("line\nnext"), "line next");
    }
}
