#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn positive_usize_rejects_zero() {
        assert!(positive_usize(Some("0".to_owned())).is_none());
        assert!(positive_usize(Some("-1".to_owned())).is_none());
    }

    #[test]
    fn positive_usize_accepts_valid_values() {
        assert_eq!(positive_usize(Some("1".to_owned())), Some(1));
        assert_eq!(positive_usize(Some("100".to_owned())), Some(100));
    }

    #[test]
    fn positive_usize_rejects_invalid_parse() {
        assert!(positive_usize(Some("not_a_number".to_owned())).is_none());
        assert!(positive_usize(None).is_none());
    }

    #[test]
    fn positive_u64_rejects_zero() {
        assert!(positive_u64(Some("0".to_owned())).is_none());
    }

    #[test]
    fn positive_u64_accepts_valid_values() {
        assert_eq!(positive_u64(Some("1".to_owned())), Some(1));
        assert_eq!(positive_u64(Some("120".to_owned())), Some(120));
    }

    #[test]
    fn setting_at_extracts_string_value() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("settings.json");
        let settings = json!({"model":"claude-opus-4","other":123});
        std::fs::write(&path, serde_json::to_string(&settings).unwrap())
            .expect("write settings");

        let result = setting_at(&path, "model");
        assert_eq!(result, Some("claude-opus-4".to_owned()));
    }

    #[test]
    fn setting_at_returns_none_for_missing_key() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("settings.json");
        std::fs::write(&path, r#"{"model":"claude-opus-4"}"#).expect("write settings");

        assert!(setting_at(&path, "missing_key").is_none());
    }

    #[test]
    fn setting_at_returns_none_for_non_string_value() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("settings.json");
        let settings = json!({"value":123});
        std::fs::write(&path, serde_json::to_string(&settings).unwrap())
            .expect("write settings");

        assert!(setting_at(&path, "value").is_none());
    }

    #[test]
    fn setting_at_skips_empty_strings() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("settings.json");
        let settings = json!({"model":""});
        std::fs::write(&path, serde_json::to_string(&settings).unwrap())
            .expect("write settings");

        assert!(setting_at(&path, "model").is_none());
    }

    #[test]
    fn setting_at_returns_none_for_missing_file() {
        let result = setting_at(std::path::Path::new("/nonexistent/settings.json"), "key");
        assert!(result.is_none());
    }

    #[test]
    fn subscription_limits_new_validates_max_processes() {
        assert!(SubscriptionLimits::new(0, 60).is_err());
        assert!(SubscriptionLimits::new(1, 60).is_ok());
    }

    #[test]
    fn subscription_limits_new_validates_timeout() {
        assert!(SubscriptionLimits::new(5, 0).is_err());
        assert!(SubscriptionLimits::new(5, 60).is_ok());
    }
}
