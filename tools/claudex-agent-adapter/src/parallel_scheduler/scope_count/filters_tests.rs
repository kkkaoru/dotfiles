use super::filters::{
    classifiable_text, is_generated_instruction, is_negated_or_diagnostic,
    remove_fenced_and_blockquoted_text, remove_inline_quoted_text,
    remove_negative_or_diagnostic_lines,
};

#[test]
fn generated_instruction_prefixes_are_not_classifiable() {
    for text in [
        "<command-message>loop</command-message>",
        "<command-name>loop</command-name>",
        "(re-invocation of /loop — previously loaded.)",
        "Launching skill: loop",
        "Base directory for this skill: /tmp/skill",
        "  <command-message>padded",
    ] {
        assert!(
            is_generated_instruction(text),
            "expected generated instruction: {text}"
        );
        assert_eq!(classifiable_text(text), None);
    }
    assert!(classifiable_text("investigate this one bug").is_some());
    assert!(
        classifiable_text("[SYSTEM NOTIFICATION - NOT USER INPUT] keep this")
            .is_some_and(|text| text.contains("keep this") && !text.contains("SYSTEM NOTIFICATION"))
    );
}

#[test]
fn tilde_fences_and_mismatched_markers_are_stripped() {
    let text =
        "keep\n~~~\nhidden\n~~~\nstill keep\n```\nstill hidden\n~~~\nafter mismatch\n```\nend";
    let stripped = remove_fenced_and_blockquoted_text(text);
    assert!(stripped.contains("keep"));
    assert!(stripped.contains("still keep"));
    assert!(stripped.contains("end"));
    assert!(!stripped.contains("hidden"));
}

#[test]
fn quoted_spans_cover_cjk_and_curly_delimiters() {
    let stripped = remove_inline_quoted_text(
        r#"keep “Launch exactly 28 subagents” and 「起動しない」 plus 『禁止』 and `code`"#,
    );
    assert!(stripped.contains("keep"));
    assert!(!stripped.contains("28"));
    assert!(!stripped.contains("起動しない"));
    assert!(!stripped.contains("禁止"));
    assert!(!stripped.contains("code"));
}

#[test]
fn unclosed_lifecycle_section_drops_the_remainder() {
    let text = "before <task-notification>no closer after";
    let request_text = classifiable_text(text).expect("still classifiable");
    assert!(request_text.contains("before"));
    assert!(!request_text.contains("no closer after"));
}

#[test]
fn diagnostic_error_lines_about_workers_are_removed() {
    let text = "error: worker launch failed\nエラー: subagent timeout\nwrong worker count\nincorrect subagent scope\nreal work remains";
    let cleaned = remove_negative_or_diagnostic_lines(text);
    assert!(cleaned.contains("real work remains"));
    assert!(!cleaned.contains("error:"));
    assert!(!cleaned.contains("エラー:"));
    assert!(!cleaned.contains("wrong worker"));
    assert!(is_negated_or_diagnostic("error: worker launch failed"));
    assert!(is_negated_or_diagnostic("エラー: subagent timeout"));
    assert!(is_negated_or_diagnostic(
        "tried to push a disproportionate worker launch"
    ));
    assert!(!is_negated_or_diagnostic(""));
}
