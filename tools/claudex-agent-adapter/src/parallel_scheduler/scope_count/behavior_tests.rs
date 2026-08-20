use std::time::Duration;

use super::{
    super::{ParallelScheduler, SchedulerConfig},
    actions::count_for_content,
    declines_delegation, has_classifiable_user_turn, has_parallel_scope, independent_scope_count,
    is_substantive_work, needs_single_worker,
    test_support::{messages_request, real_three_scope_message},
};

#[test]
fn action_list_contexts_cover_acceptance_and_nested_items() {
    assert_eq!(
        count_for_content("Tasks:\n- Implement one\n  continuation\n- Fix two"),
        2
    );
    assert_eq!(
        count_for_content("Acceptance criteria:\n- Implement one\n- Fix two"),
        1
    );
    assert_eq!(count_for_content("- note\n- example"), 1);
}

#[test]
fn incident_skill_meta_and_task_reminders_do_not_inject_twenty_eight_scopes() {
    let skill_bullets = (1..=28)
        .map(|index| format!("- generated skill rule {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let request = messages_request(vec![
        real_three_scope_message(),
        serde_json::json!({
            "role": "assistant",
            "content": [{"type":"tool_use", "id":"skill-1", "name":"Skill"}]
        }),
        serde_json::json!({
            "role": "user",
            "content": [{"type":"tool_result", "tool_use_id":"skill-1", "content":"Launching skill: loop"}]
        }),
        serde_json::json!({
            "role": "user",
            "isMeta": true,
            "sourceToolUseID": "skill-1",
            "content": "(Re-invocation of /loop — previously loaded.)"
        }),
        serde_json::json!({
            "role": "user",
            "content": [{"type":"text", "text":format!("# /loop\n{skill_bullets}\n## Input\nreal task")}]
        }),
        serde_json::json!({
            "role": "user",
            "content": [
                {"type":"tool_result", "tool_use_id":"agent-1", "content":"done"},
                {"type":"task_reminder", "content":[], "itemCount":0}
            ]
        }),
    ]);

    assert_eq!(independent_scope_count(&request), 3);
    assert!(has_classifiable_user_turn(&request));
    let guidance = ParallelScheduler::for_tests().guidance_for_request(&request);
    assert!(guidance.contains("Launch exactly 3 ordinary SubAgents"));
    assert!(!guidance.contains("Launch exactly 28 ordinary SubAgents"));
}

#[test]
fn quoted_exact_twenty_eight_diagnostic_is_not_a_scope_count() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Why did it say “Launch exactly 28 subagents”?"
    })]);
    assert_eq!(independent_scope_count(&request), 1);
    assert!(
        !ParallelScheduler::for_tests()
            .guidance_for_request(&request)
            .contains("exactly 28")
    );
}

#[test]
fn negated_worker_number_is_not_a_scope_count() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Do not launch 28 workers; investigate this one bug."
    })]);
    assert_eq!(independent_scope_count(&request), 1);
    assert!(declines_delegation(&request));
}

#[test]
fn fenced_and_blockquoted_lists_are_not_action_scopes() {
    let fenced = (1..=28)
        .map(|index| format!("- generated-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":format!("Investigate one bug.\n```yaml\n{fenced}\n```\n> - quoted one\n> - quoted two")
    })]);
    assert_eq!(independent_scope_count(&request), 1);
}

#[test]
fn acceptance_criteria_are_not_worker_scopes() {
    let criteria = (1..=28)
        .map(|index| format!("- verify acceptance condition {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":format!("Implement one parser feature.\nAcceptance criteria:\n{criteria}")
    })]);
    assert_eq!(independent_scope_count(&request), 1);
}

#[test]
fn trailing_and_interleaved_lifecycle_sections_are_removed() {
    let fake = (1..=28)
        .map(|index| format!("- generated reminder {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":format!("Tasks:\n- implement parser\n<system-reminder>\n{fake}\n</system-reminder>\n- verify renderer")
    })]);
    assert_eq!(independent_scope_count(&request), 2);
}

#[test]
fn prefixed_system_notification_is_removed() {
    let request = messages_request(vec![
        real_three_scope_message(),
        serde_json::json!({
            "role":"user",
            "content":"[SYSTEM NOTIFICATION - NOT USER INPUT]\n<task-notification>\nTasks:\n- implement fake one\n- implement fake two\n- implement fake three\n- implement fake four\n</task-notification>"
        }),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn same_content_command_marker_and_generic_skill_dump_are_removed() {
    let request = messages_request(vec![
        real_three_scope_message(),
        serde_json::json!({
            "role":"user",
            "content":[
                {"type":"text", "text":"<command-message>loop</command-message>"},
                {"type":"text", "text":"Skill instructions\nBase directory for this skill: /tmp/skill\nTasks:\n- implement fake one\n- implement fake two\n- implement fake three\n- implement fake four"}
            ]
        }),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn literal_generated_marker_in_ordinary_user_prose_is_preserved() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Review the literal <command-message> marker in these tasks:\n- implement parser handling\n- verify renderer handling"
    })]);
    assert_eq!(independent_scope_count(&request), 2);
}

#[test]
fn unrelated_old_skill_does_not_hide_a_real_slash_command_document() {
    let request = messages_request(vec![
        serde_json::json!({
            "role":"assistant",
            "content":[{"type":"tool_use", "id":"skill-old", "name":"Skill"}]
        }),
        serde_json::json!({"role":"user", "content":"ordinary intervening request"}),
        serde_json::json!({"role":"assistant", "content":"ack"}),
        serde_json::json!({
            "role":"user",
            "content":"# /local-command documentation\n## Input\n- syntax\n- behavior\n- tests"
        }),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn nested_bullets_do_not_add_scopes() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Tasks:\n- implement parser\n  - verify edge A\n  - verify edge B\n- test renderer"
    })]);
    assert_eq!(independent_scope_count(&request), 2);
}

#[test]
fn first_indented_list_item_is_not_dropped_when_later_items_are_flush_left() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Tasks:\n  - inspect dash\n* test star\n・ verify dot\n1. implement numbered"
    })]);
    assert_eq!(independent_scope_count(&request), 4);
}

#[test]
fn example_in_an_entity_name_does_not_turn_actions_into_an_example_section() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"架空組織 Example Labs を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向"
    })]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn action_list_wins_over_a_larger_constraints_list() {
    let constraints = (1..=10)
        .map(|index| format!("- do not violate constraint {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":format!("Tasks:\n- implement parser\n- verify renderer\n- test integration\nConstraints:\n{constraints}")
    })]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn configured_max_workers_caps_exact_guidance() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: 4,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Tasks:\n- implement one\n- implement two\n- implement three\n- implement four\n- implement five\n- implement six\n- implement seven"
    })]);
    let decision = scheduler.decision_for_request(&request);
    let guidance = scheduler.guidance_for_request(&request);
    assert_eq!(decision.target_workers, 4);
    assert!(guidance.contains("Launch exactly 4 ordinary SubAgents"));
    assert!(!guidance.contains("Launch exactly 7 ordinary SubAgents"));
}

#[test]
fn old_atomic_lookup_does_not_force_latest_substantive_turn_to_one() {
    let request = messages_request(vec![
        serde_json::json!({"role":"user", "content":"gh pr view 12"}),
        serde_json::json!({"role":"assistant", "content":"done"}),
        serde_json::json!({"role":"user", "content":"Investigate the parser regression"}),
    ]);
    assert!(!needs_single_worker(&request));
    assert!(is_substantive_work(&request));
}

#[test]
fn missing_user_text_does_not_invent_parallel_scopes() {
    let request = messages_request(vec![serde_json::json!({"role":"assistant", "content":[]})]);
    assert_eq!(independent_scope_count(&request), 0);
    assert!(!has_parallel_scope(&request));
}

#[test]
fn stop_remaining_workers_is_current_user_intent() {
    let request = messages_request(vec![
        real_three_scope_message(),
        serde_json::json!({"role":"user", "content":"Stop remaining SubAgents"}),
    ]);
    assert_eq!(independent_scope_count(&request), 1);
    assert!(declines_delegation(&request));
}

#[test]
fn new_plain_follow_up_replaces_an_older_parallel_request() {
    let request = messages_request(vec![
        real_three_scope_message(),
        serde_json::json!({"role":"assistant", "content":"working"}),
        serde_json::json!({"role":"user", "content":"進捗を教えて"}),
    ]);
    assert_eq!(independent_scope_count(&request), 1);
    assert!(!is_substantive_work(&request));
}

#[test]
fn explicit_stated_cardinality_is_the_scope_target() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Launch 4 independent scopes for this investigation."
    })]);
    assert_eq!(independent_scope_count(&request), 4);
    let scheduler = ParallelScheduler::for_tests();
    let decision = scheduler.decision_for_request(&request);
    assert_eq!(decision.target_workers, 4);
    assert!(
        scheduler
            .guidance_for_request(&request)
            .contains("Launch exactly 4 ordinary SubAgents")
    );
}

#[test]
fn explicit_cardinality_wins_over_a_shorter_action_list() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"Use 4 workers.\n- implement parser\n- verify renderer"
    })]);
    assert_eq!(independent_scope_count(&request), 4);
}

#[test]
fn single_worker_and_substantive_gates_cover_decline_and_parallel_counts() {
    let declined = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":"do not delegate this lookup"
    })]);
    assert!(declines_delegation(&declined));
    assert!(!needs_single_worker(&declined));
    assert!(!is_substantive_work(&declined));

    let parallel = messages_request(vec![real_three_scope_message()]);
    assert!(!needs_single_worker(&parallel));
    assert!(is_substantive_work(&parallel));
}
