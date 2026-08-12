#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skill_payload_is_false_for_an_out_of_range_index() {
        assert!(!is_generated_skill_payload(&[], 0));
    }

    #[test]
    fn skill_payload_is_false_for_blank_text() {
        let messages = vec![json!({"role": "user", "content": "   "})];
        assert!(!is_generated_skill_payload(&messages, 0));
    }

    #[test]
    fn skill_payload_requires_both_the_command_marker_and_the_skill_base_line() {
        let marker_only = vec![json!({
            "role": "user",
            "content": "<command-message>loop</command-message>\nno base line here"
        })];
        assert!(!is_generated_skill_payload(&marker_only, 0));

        let base_only = vec![json!({
            "role": "user",
            "content": "Base directory for this skill: /tmp/skill\nno marker here"
        })];
        assert!(!is_generated_skill_payload(&base_only, 0));

        let both = vec![json!({
            "role": "user",
            "content": "Launching skill: loop\nBase directory for this skill: /tmp/skill"
        })];
        assert!(is_generated_skill_payload(&both, 0));
    }

    #[test]
    fn skill_payload_recognizes_a_skill_document_that_follows_an_invocation() {
        let messages = vec![
            json!({
                "role": "assistant",
                "content": [{"type": "tool_use", "name": "Skill"}]
            }),
            json!({
                "role": "user",
                "content": "# /loop\n## Input\nDo the thing"
            }),
        ];
        assert!(is_generated_skill_payload(&messages, 1));

        let unrelated = vec![
            json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]}),
            json!({"role": "user", "content": "# /loop\n## Input\nDo the thing"}),
        ];
        assert!(!is_generated_skill_payload(&unrelated, 1));
    }

    #[test]
    fn exact_generated_command_line_matches_each_recognized_prefix() {
        assert!(is_exact_generated_command_line(
            "(Re-invocation of /loop — previously loaded.)"
        ));
        assert!(is_exact_generated_command_line("Launching skill: loop"));
        assert!(!is_exact_generated_command_line("investigate this bug"));
    }

    #[test]
    fn generated_chain_message_requires_every_array_block_to_be_generated() {
        let all_generated = json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "<command-message>loop</command-message>"},
                {"isMeta": true, "type": "text", "text": "hidden"}
            ]
        });
        assert!(is_generated_chain_message(&all_generated));

        let mixed = json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "<command-message>loop</command-message>"},
                {"type": "text", "text": "real user follow-up"}
            ]
        });
        assert!(!is_generated_chain_message(&mixed));

        let empty_array = json!({"role": "user", "content": []});
        assert!(!is_generated_chain_message(&empty_array));
    }

    #[test]
    fn looks_like_skill_document_matches_either_a_base_line_or_a_slash_command_header() {
        assert!(looks_like_skill_document(
            "intro\nBase directory for this skill: /tmp/skill\nmore"
        ));
        assert!(looks_like_skill_document("# /loop\n## Input\ndetails"));
        assert!(looks_like_skill_document("# /loop\n# Input\ndetails"));
        assert!(!looks_like_skill_document("just a regular user message"));
        assert!(!looks_like_skill_document(
            "# /loop without an input section"
        ));
    }
}
