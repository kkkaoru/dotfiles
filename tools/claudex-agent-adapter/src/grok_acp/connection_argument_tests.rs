    use super::{normalize_launch_effort, substitute_configured_argument};

    #[test]
    fn substitutes_model_and_thinking_effort() {
        assert_eq!(normalize_launch_effort("max"), "xhigh");
        assert_eq!(normalize_launch_effort("high"), "high");
        let rendered =
            substitute_configured_argument("--thinking", "qwen/qwen3.8-max", Some("high")).unwrap();
        assert_eq!(rendered, "--thinking");
        assert_eq!(
            substitute_configured_argument("{effort}", "m", Some("max")).unwrap(),
            "xhigh"
        );
        assert_eq!(
            substitute_configured_argument("-m", "qwen/qwen3.8-max", None).unwrap(),
            "-m"
        );
        assert_eq!(
            substitute_configured_argument("{model}", "qwen/qwen3.8-max", None).unwrap(),
            "qwen/qwen3.8-max"
        );
        assert!(substitute_configured_argument("{effort}", "m", None).is_err());
    }
