use std::time::Duration;

use super::*;

fn request(content: Vec<serde_json::Value>) -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: serde_json::Value::Null,
        messages: vec![serde_json::json!({
            "role": "assistant",
            "content": content,
        })],
        tools: Vec::new(),
        stream: false,
        output_config: serde_json::json!({}),
        metadata: serde_json::json!({"user_id": "model-diversity-tests"}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn config() -> SchedulerConfig {
    SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    }
}

fn agent(id: &str, model: &str) -> serde_json::Value {
    agent_with_type(id, model, "general-purpose")
}

fn agent_with_type(id: &str, model: &str, subagent_type: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "tool_use",
        "id": id,
        "name": "cc_Agent_0",
        "input": {
            "description": id,
            "prompt": format!("work on {id}"),
            "subagent_type": subagent_type,
            "claudex_model": model,
        },
    })
}

#[test]
fn treats_gpt_variants_as_one_family_and_requires_grok_for_diversity() {
    let scheduler = ParallelScheduler::new(config());
    let decision = scheduler.decision_for_request(&request(vec![
        agent("luna", "gpt-5.6-luna"),
        agent("spark", "gpt-5.3-codex-spark"),
        agent("grok", "grok-4.5"),
    ]));

    assert_eq!(decision.active_workers, 3);
    assert_eq!(decision.active_model_families, 2);
    assert!(!decision.needs_model_diversity);
    assert_eq!(decision.needs_more_workers, 0);
}

#[test]
fn keeps_a_three_worker_target_and_recovers_the_two_worker_floor() {
    let scheduler = ParallelScheduler::new(config());
    let decision =
        scheduler.decision_for_request(&request(vec![agent("only-worker", "gpt-5.6-luna")]));

    assert_eq!(decision.target_workers, 3);
    assert_eq!(decision.active_workers, 1);
    assert_eq!(decision.needs_more_workers, 2);
    assert!(decision.active_floor_breached);
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("Only one active lane remains"))
    );
}

#[test]
fn keeps_the_ten_minute_default_and_reassesses_after_a_completion() {
    assert_eq!(
        SchedulerConfig::default().reassess_interval,
        Duration::from_secs(600)
    );

    let scheduler = ParallelScheduler::new(config());
    let initial = request(vec![
        agent("luna", "gpt-5.6-luna"),
        agent("grok", "grok-4.5"),
        agent("spark", "gpt-5.3-codex-spark"),
    ]);
    let after_completion = MessagesRequest {
        messages: vec![serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "tool_result", "tool_use_id": "spark", "content": "done"},
                agent("luna", "gpt-5.6-luna"),
                agent("grok", "grok-4.5"),
            ],
        })],
        ..initial.clone()
    };

    let first = scheduler.decision_for_request(&initial);
    assert!(first.guidance(&scheduler.config()).contains("Re-evaluate"));

    let completed = scheduler.decision_for_request(&after_completion);
    assert_eq!(completed.completed_recently, 1);
    assert!(
        completed
            .guidance(&scheduler.config())
            .contains("Re-evaluate")
    );
}

#[test]
fn reuses_an_active_compatible_worker_instead_of_churning_sessions() {
    let scheduler = ParallelScheduler::new(config());
    let decision = scheduler.decision_for_request(&request(vec![
        agent("luna", "gpt-5.6-luna"),
        agent("grok", "grok-4.5"),
    ]));

    assert!(
        decision
            .guidance(&scheduler.config())
            .contains("Prefer reusing compatible completed workers via SendMessage")
    );
}

#[test]
fn custom_advisor_does_not_consume_ordinary_worker_floor_or_diversity() {
    let scheduler = ParallelScheduler::new(config());
    let decision = scheduler.decision_for_request(&request(vec![agent_with_type(
        "advisor",
        "claude-fable-5",
        "custom-advisor",
    )]));

    assert_eq!(decision.active_workers, 0);
    assert_eq!(decision.active_model_families, 0);
    assert_eq!(decision.needs_more_workers, 0);
    assert!(!decision.active_floor_breached);
    assert!(!decision.needs_model_diversity);
    assert!(decision.actions.is_empty());
}
