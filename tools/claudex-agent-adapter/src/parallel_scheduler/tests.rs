use super::*;
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::Instant,
};

fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    TEST_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("parallel scheduler env test lock")
}

fn messages(payload: &[serde_json::Value]) -> MessagesRequest {
    MessagesRequest {
        model: "main".to_owned(),
        system: serde_json::json!(null),
        messages: payload.to_vec(),
        tools: Vec::new(),
        stream: false,
        output_config: serde_json::json!({}),
        metadata: serde_json::json!({ "user_id":"user" }),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn tool_use(id: &str, name: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "type":"tool_use",
        "id":id,
        "name":name,
        "input":{"claudex_model":model}
    })
}

fn clear_scheduler_env() {
    unsafe {
        std::env::remove_var(SUBAGENT_MIN_PARALLEL_ENV);
        std::env::remove_var(SUBAGENT_MAX_PARALLEL_ENV);
        std::env::remove_var(SUBAGENT_MAX_CONCURRENT_SUBAGENTS_ENV);
        std::env::remove_var(SUBAGENT_ACTIVE_FLOOR_ENV);
        std::env::remove_var(SUBAGENT_REEVALUATE_ON_COMPLETION_ENV);
        std::env::remove_var(SUBAGENT_REASSESS_INTERVAL_SECONDS_ENV);
        std::env::remove_var(SUBAGENT_MIN_MODEL_FAMILIES_ENV);
        std::env::remove_var(SUBAGENT_REUSE_ENV);
        std::env::remove_var(SUBAGENT_CLEANUP_ON_EXIT_ENV);
    }
}

#[test]
fn evaluates_subagent_floor_and_reuse_signals() {
    let scheduler = ParallelScheduler::for_tests();
    let state = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[tool_use("t1","cc_Agent_0","gpt-5.6-sol")],
    })]);
    let first = scheduler.decision_for_request(&state);
    assert_eq!(first.active_workers, 1);
    assert!(first.active_floor_breached);
    assert_eq!(first.needs_more_workers, 1);
    assert!(first.guidance(&scheduler.config()).contains("Re-evaluate"));
}

#[test]
fn parses_batch_launches() {
    let scheduler = ParallelScheduler::for_tests();
    let state = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {
                "type":"tool_use",
                "id":"batch-1",
                "name":"cc_Agent_batch_0",
                "input":{"tasks":[
                    {"prompt":"a","claudex_model":"gpt-5.6-sol"},
                    {"prompt":"b","claudex_model":"grok-4.5"},
                    {"prompt":"c","claudex_model":"grok-4.5"},
                ]}
            }
        ]
    })]);
    let decision = scheduler.decision_for_request(&state);
    assert_eq!(decision.active_workers, 3);
    assert_eq!(decision.active_model_families, 2);
    assert_eq!(decision.needs_more_workers, 0);
}

#[test]
fn healthy_active_floor_is_not_marked_as_breached() {
    let scheduler = ParallelScheduler::for_tests();
    let state = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t2","cc_Agent_0","grok-4.5"),
        ]
    })]);

    let decision = scheduler.decision_for_request(&state);

    assert_eq!(decision.active_workers, 2);
    assert!(!decision.active_floor_breached);
}

#[test]
fn ignores_custom_advisor_tasks_for_concurrency_tracking() {
    let snapshot = core::analyze_subagent_work(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {
                "type":"tool_use",
                "id":"advisor",
                "name":"Task",
                "input":{
                    "subagent_type":"custom-advisor",
                    "claudex_model":"claude-fable-5",
                }
            },
            {
                "type":"tool_use",
                "id":"worker",
                "name":"cc_Agent_0",
                "input":{"claudex_model":"gpt-5.6-sol"}
            }
        ]
    })]);
    assert_eq!(snapshot.active_count(), 1);
    assert_eq!(snapshot.active_model_families(), 1);

    let batch_snapshot = core::analyze_subagent_work(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {
                "type":"tool_use",
                "id":"batch",
                "name":"cc_Agent_batch_0",
                "input":{
                    "tasks":[
                        {"prompt":"adv","subagent_type":"custom-advisor","claudex_model":"claude-fable-5"},
                        {"prompt":"work","subagent_type":"cc-agent","claudex_model":"grok-4.5"},
                        {"prompt":"work2","claudex_model":"gpt-5.6-sol"}
                    ]
                }
            }
        ]
    })]);
    assert_eq!(batch_snapshot.active_count(), 2);
    assert_eq!(batch_snapshot.active_model_families(), 2);
}

#[test]
fn preserves_reassessment_baseline() {
    let config = SchedulerConfig {
        reassess_interval: std::time::Duration::from_millis(200),
        ..SchedulerConfig::default()
    };
    let scheduler = ParallelScheduler::new(config);
    let state = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t2","cc_Agent_0","grok-4.5"),
            tool_use("t3","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let first = scheduler.decision_for_request(&state);
    assert!(
        first.guidance(&scheduler.config()).contains("Re-evaluate"),
        "first request should request a reassessment"
    );
    std::thread::sleep(std::time::Duration::from_millis(120));
    let _ignore = scheduler.decision_for_request(&state);
    std::thread::sleep(std::time::Duration::from_millis(220));
    let third = scheduler.decision_for_request(&state);
    assert!(
        third.guidance(&scheduler.config()).contains("Re-evaluate"),
        "reassessment should run after interval even with an intervening fast cycle",
    );
}

#[test]
fn guidance_includes_completion_followup_when_workers_finish() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let first = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t2","cc_Agent_0","grok-4.5"),
            tool_use("t3","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let second = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {
                "type":"tool_result",
                "tool_use_id":"t3",
                "content":"done",
            },
            tool_use("t2","cc_Agent_0","grok-4.5"),
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&second);
    assert_eq!(decision.completed_recently, 1);
    assert!(
        decision
            .guidance(&scheduler.config())
            .contains("Worker-cycle")
    );
    assert!(
        decision
            .guidance(&scheduler.config())
            .contains("re-issue same-scope tasks")
    );
}

#[test]
fn one_active_worker_triggers_interruption_and_replacement_protocol() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let first = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t2","cc_Agent_0","grok-4.5"),
        ]
    })]);
    let second = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {
                "type":"tool_result",
                "tool_use_id":"t2",
                "content":"done",
            },
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&second);
    assert_eq!(decision.active_workers, 1);
    assert!(decision.active_floor_breached);
    assert!(decision.needs_more_workers >= 1);
    let guidance = decision.guidance(&scheduler.config());
    assert!(guidance.contains("Only one active lane remains"));
    assert!(guidance.contains("re-issue same-scope"));
}

#[test]
fn stale_single_worker_is_replaced_on_reassessment_tick() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_millis(200),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let steady = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let _ = scheduler.decision_for_request(&steady);
    std::thread::sleep(std::time::Duration::from_millis(220));
    let decision = scheduler.decision_for_request(&steady);
    assert_eq!(decision.active_workers, 1);
    assert!(decision.active_floor_breached);
    assert!(decision.needs_more_workers >= 1);
    let guidance = decision.guidance(&scheduler.config());
    assert!(guidance.contains("Re-evaluate"));
    assert!(guidance.contains("interrupt stale work"));
    assert!(guidance.contains("then continue"));
}

#[test]
fn config_reads_environmental_defaults() {
    let _lock = env_test_lock();
    clear_scheduler_env();
    unsafe {
        std::env::set_var(SUBAGENT_MIN_PARALLEL_ENV, "5");
        std::env::set_var(SUBAGENT_ACTIVE_FLOOR_ENV, "1");
    }
    let config = SchedulerConfig::parse();
    assert_eq!(config.min_parallel_workers, 5);
    assert_eq!(config.active_floor, 2);
    clear_scheduler_env();
}

#[test]
fn config_reads_legacy_max_parallel_env() {
    let _lock = env_test_lock();
    clear_scheduler_env();
    unsafe {
        std::env::set_var(SUBAGENT_MIN_PARALLEL_ENV, "invalid");
        std::env::set_var(SUBAGENT_MAX_CONCURRENT_SUBAGENTS_ENV, "invalid");
        std::env::set_var(SUBAGENT_MAX_PARALLEL_ENV, "7");
    }
    let config = SchedulerConfig::parse();
    assert_eq!(config.min_parallel_workers, 3);
    assert_eq!(config.max_parallel_workers, 7);
    clear_scheduler_env();
}

#[test]
fn min_parallel_env_takes_precedence_over_legacy_max_parallel_env() {
    let _lock = env_test_lock();
    clear_scheduler_env();
    unsafe {
        std::env::set_var(SUBAGENT_MIN_PARALLEL_ENV, "4");
        std::env::set_var(SUBAGENT_MAX_PARALLEL_ENV, "9");
    }
    let config = SchedulerConfig::parse();
    assert_eq!(config.min_parallel_workers, 4);
    clear_scheduler_env();
}

#[test]
fn max_parallel_below_active_floor_clamps_active_floor() {
    let _lock = env_test_lock();
    clear_scheduler_env();
    unsafe {
        std::env::set_var(SUBAGENT_MAX_PARALLEL_ENV, "3");
        std::env::set_var(SUBAGENT_ACTIVE_FLOOR_ENV, "5");
    }
    let config = SchedulerConfig::parse();
    assert_eq!(config.max_parallel_workers, 3);
    assert_eq!(config.active_floor, 2);
    assert_eq!(config.min_parallel_workers, 3);
    clear_scheduler_env();
}

#[test]
fn capacity_action_handles_inverted_manual_worker_bounds() {
    let config = SchedulerConfig {
        min_parallel_workers: 8,
        max_parallel_workers: 3,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    };
    let mut decision = SchedulerDecision::no_action();
    decision.active_workers = 0;

    policy::apply_capacity_actions(&mut decision, 6, &config);

    assert_eq!(decision.target_workers, 3);
    assert_eq!(decision.needs_more_workers, 3);
}

#[test]
fn min_parallel_value_above_max_is_clamped_to_max() {
    let _lock = env_test_lock();
    clear_scheduler_env();
    unsafe {
        std::env::set_var(SUBAGENT_MAX_PARALLEL_ENV, "3");
        std::env::set_var(SUBAGENT_MIN_PARALLEL_ENV, "6");
    }
    let config = SchedulerConfig::parse();
    assert_eq!(config.max_parallel_workers, 3);
    assert_eq!(config.min_parallel_workers, 3);
    clear_scheduler_env();
}

#[test]
fn when_one_active_worker_remains_prompt_interrupts_and_replaces() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let first = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t2","cc_Agent_0","grok-4.5"),
            tool_use("t3","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t4","cc_Agent_0","grok-4.5"),
        ]
    })]);
    let second = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {
                "type":"tool_result",
                "tool_use_id":"t2",
                "content":"done"
            },
            {
                "type":"tool_result",
                "tool_use_id":"t3",
                "content":"done"
            },
            {
                "type":"tool_result",
                "tool_use_id":"t4",
                "content":"done"
            },
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&second);
    assert_eq!(decision.active_workers, 1);
    assert_eq!(decision.completed_recently, 3);
    assert!(decision.active_floor_breached);
    assert!(decision.needs_more_workers >= 1);
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("interrupt stale work"))
    );
}

#[test]
fn increases_floor_with_explicit_request_structure() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let request = messages(&[
        serde_json::json!({"role":"user","content":"\n- 項目Aの分析\n- 項目Bの検証\n- 項目Cの比較"}),
        serde_json::json!({
            "role":"assistant",
            "content":[tool_use("t1","cc_Agent_0","gpt-5.6-sol")]
        }),
    ]);
    let decision = scheduler.decision_for_request(&request);
    assert_eq!(decision.target_workers, 3);
    assert_eq!(
        decision.needs_more_workers,
        decision.target_workers - decision.active_workers
    );
    assert!(
        decision
            .guidance(&scheduler.config())
            .contains("target concurrency is")
    );
}

#[test]
fn leaves_a_single_indivisible_lane_steady_between_rebalance_events() {
    let scheduler = ParallelScheduler::for_tests();
    let state = messages(&[
        serde_json::json!({
            "role":"user",
            "content":"gh pr view https://github.com/example/repo/pull/1",
        }),
        serde_json::json!({
            "role":"assistant",
            "content":[tool_use("t1","cc_Agent_0","gpt-5.6-sol")],
        }),
    ]);
    let initial = scheduler.decision_for_request(&state);
    assert_eq!(
        initial.target_workers, 1,
        "a single bounded request starts with one worker"
    );

    let steady = scheduler.decision_for_request(&state);
    assert_eq!(steady.active_workers, 1);
    assert_eq!(steady.target_workers, 1);
    assert_eq!(steady.needs_more_workers, 0);
    assert!(!steady.active_floor_breached);
    assert!(
        !steady
            .actions
            .iter()
            .any(|action| action.contains("Only one active lane remains"))
    );
}

#[test]
fn single_gh_pr_lookup_schedules_exactly_one_worker_on_its_initial_cycle() {
    let scheduler = ParallelScheduler::for_tests();
    let request = messages(&[serde_json::json!({
        "role": "user",
        "content": "gh コマンドで https://github.com/avita-co-jp/avatar-infra/pull/74 の情報を取得して",
    })]);

    let decision = scheduler.decision_for_request(&request);

    assert_eq!(decision.target_workers, 1);
    assert_eq!(decision.needs_more_workers, 1);
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("Launch at least 1")),
        "an indivisible `gh pr view` request must launch one worker"
    );
    assert!(
        scheduler
            .guidance_for_request(&request)
            .contains("Task-shape: one bounded or indivisible scope detected. Launch exactly one")
    );
}

#[test]
fn single_gh_pr_lookup_does_not_expand_after_its_worker_starts() {
    let scheduler = ParallelScheduler::for_tests();
    let request = messages(&[
        serde_json::json!({
            "role": "user",
            "content": "gh コマンドで https://github.com/avita-co-jp/avatar-infra/pull/74 の情報を取得して",
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [tool_use("pr-74", "cc_Agent_0", "gpt-5.6-sol")],
        }),
    ]);

    let decision = scheduler.decision_for_request(&request);

    assert_eq!(decision.active_workers, 1);
    assert_eq!(decision.target_workers, 1);
    assert_eq!(decision.needs_more_workers, 0);
    assert!(!decision.active_floor_breached);
    assert!(
        !decision
            .actions
            .iter()
            .any(|action| action.contains("Launch at least")),
        "an indivisible `gh pr view` request must not be expanded into duplicate workers"
    );
    assert!(
        scheduler
            .guidance_for_request(&request)
            .contains("selected_workers is a capacity pool, not a launch count")
    );
}

#[test]
fn explicit_parallel_request_keeps_multi_worker_fanout_without_list_markers() {
    let scheduler = ParallelScheduler::for_tests();
    let request = messages(&[
        serde_json::json!({
            "role": "user",
            "content": "AVITA株式会社を複数のSubAgentで並列で調査してください",
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [tool_use("company", "cc_Agent_0", "gpt-5.6-sol")],
        }),
    ]);

    let decision = scheduler.decision_for_request(&request);

    assert!(decision.target_workers >= 3);
    assert_eq!(decision.needs_more_workers, decision.target_workers - 1);
    assert!(
        scheduler
            .guidance_for_request(&request)
            .contains("Task-shape: multiple independent scopes detected")
    );
}

#[test]
fn independent_research_scopes_still_request_parallel_workers() {
    let scheduler = ParallelScheduler::for_tests();
    let request = messages(&[
        serde_json::json!({
            "role": "user",
            "content": "AVITA株式会社を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向",
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [tool_use("company", "cc_Agent_0", "gpt-5.6-sol")],
        }),
    ]);

    let decision = scheduler.decision_for_request(&request);

    assert_eq!(decision.active_workers, 1);
    assert_eq!(decision.target_workers, 3);
    assert_eq!(decision.needs_more_workers, 2);
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("Launch at least 2")),
        "three non-overlapping research scopes should retain useful fan-out"
    );
}

#[test]
fn multi_scope_completion_reassesses_only_the_unfinished_lanes() {
    let scheduler = ParallelScheduler::for_tests();
    let first = messages(&[
        serde_json::json!({
            "role": "user",
            "content": "AVITA株式会社を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向",
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [
                tool_use("company", "cc_Agent_0", "gpt-5.6-sol"),
                tool_use("funding", "cc_Agent_0", "grok-4.5"),
                tool_use("market", "cc_Agent_0", "gpt-5.6-sol"),
            ],
        }),
    ]);
    let after_market = messages(&[
        serde_json::json!({
            "role": "user",
            "content": "AVITA株式会社を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向",
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [
                {"type":"tool_result", "tool_use_id":"market", "content":"done"},
                tool_use("company", "cc_Agent_0", "gpt-5.6-sol"),
                tool_use("funding", "cc_Agent_0", "grok-4.5"),
            ],
        }),
    ]);

    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&after_market);

    assert_eq!(decision.completed_recently, 1);
    assert_eq!(decision.active_workers, 2);
    assert_eq!(decision.target_workers, 3);
    assert_eq!(decision.needs_more_workers, 1);
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("immediately after completion")),
        "completion must keep the dynamic reassessment path for unfinished work"
    );
}

#[test]
fn replenishes_only_to_the_active_floor_after_completion() {
    let scheduler = ParallelScheduler::for_tests();
    let first = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
            tool_use("t2","cc_Agent_0","grok-4.5"),
            tool_use("t3","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let second = messages(&[serde_json::json!({
        "role":"assistant",
        "content":[
            {"type":"tool_result","tool_use_id":"t2","content":"done"},
            {"type":"tool_result","tool_use_id":"t3","content":"done"},
            tool_use("t1","cc_Agent_0","gpt-5.6-sol"),
        ]
    })]);
    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&second);
    assert_eq!(decision.completed_recently, 2);
    assert_eq!(decision.target_workers, 2);
    assert_eq!(decision.needs_more_workers, 1);
    assert!(decision.active_floor_breached);
}

#[test]
fn no_active_or_completed_workers_keeps_spawn_pressure_at_zero() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 3,
        max_parallel_workers: DEFAULT_MAX_PARALLEL_WORKERS,
        active_floor: 2,
        reevaluate_on_completion: true,
        reassess_interval: std::time::Duration::from_secs(600),
        min_model_families: 2,
        allow_reuse: true,
        cleanup_on_exit: true,
    });
    let request = messages(&[serde_json::json!({
        "role": "user",
        "content": "短く回答してください",
    })]);
    let decision = scheduler.decision_for_request(&request);
    assert_eq!(decision.active_workers, 0);
    assert_eq!(decision.completed_recently, 0);
    assert_eq!(decision.needs_more_workers, 0);
    assert!(decision.actions.is_empty());
}

#[test]
fn guidance_reports_completion_and_missing_model_diversity() {
    let config = SchedulerConfig::default();
    let mut decision = SchedulerDecision::no_action();
    decision.actions.push("continue work".to_owned());
    decision.target_workers = 4;
    decision.active_workers = 2;
    decision.completed_recently = 1;
    decision.active_model_families = 1;

    let guidance = decision.guidance(&config);

    assert!(guidance.contains("Worker-cycle: 1 worker"));
    assert!(guidance.contains("Model-policy: ensure at least 2 model families"));
}

#[test]
fn ignores_malformed_or_completed_subagent_tool_payloads() {
    let snapshot = core::analyze_subagent_work(&[
        serde_json::json!({
            "role": "assistant",
            "content": [
                {},
                {"type": "other"},
                {"type": "tool_result"},
                {"type": "tool_use"},
                {"type": "tool_use", "name": "cc_Agent_0"},
                {"type": "tool_use", "name": "cc_Agent_0", "id": "missing-input"},
                {"type": "tool_use", "name": "cc_Agent_0", "id": "missing-model", "input": {}},
                {"type": "tool_use", "name": "cc_Agent_batch_0", "id": "missing-tasks", "input": {}},
                {"type": "tool_use", "name": "cc_Agent_batch_0", "id": "batch", "input": {
                    "tasks": [null, {"subagent_type": "custom-advisor"}, {"prompt": "no model"}, {"claudex_model": "grok-4.5"}]
                }},
                {"type": "tool_result", "tool_use_id": "batch"},
                {"type": "tool_use", "name": "cc_Agent_0", "id": "finished", "input": {"claudex_model": "gpt-5.6-sol"}},
                {"type": "tool_result", "tool_use_id": "finished"}
            ]
        }),
        serde_json::json!({"role": "user"}),
    ]);

    assert_eq!(snapshot.active_count(), 0);
    assert_eq!(snapshot.active_model_families(), 0);
}

#[test]
fn policy_helpers_cover_early_returns_and_cleanup_choices() {
    let config = SchedulerConfig::default();
    let no_workers = core::SubagentSnapshot::default();
    let single_request = messages(&[serde_json::json!({
        "role": "user",
        "content": "gh pr view https://github.com/example/repo/pull/1",
    })]);
    let parallel_request = messages(&[serde_json::json!({
        "role": "user",
        "content": "調査を分担して並列で実行してください。\n- 概要\n- リスク",
    })]);
    let mut untouched = SchedulerDecision::no_action();

    policy::apply_reassessment_actions(&mut untouched, &no_workers, &single_request, &config, true);
    policy::apply_floor_action(&mut untouched, &single_request, &config);
    policy::apply_diversity_action(&mut untouched, &single_request, &config);
    policy::apply_reuse_actions(&mut untouched, &single_request, &config);
    policy::clear_empty_decision(&mut untouched, &no_workers);
    assert!(untouched.actions.is_empty());

    let mut completed_but_idle = SchedulerDecision::no_action();
    completed_but_idle.completed_recently = 1;
    completed_but_idle
        .actions
        .push("keep completion context".to_owned());
    policy::apply_reuse_actions(&mut completed_but_idle, &single_request, &config);
    policy::clear_empty_decision(&mut completed_but_idle, &no_workers);
    assert_eq!(completed_but_idle.actions, ["keep completion context"]);

    let mut decision = SchedulerDecision::no_action();
    decision.active_workers = 3;
    policy::apply_capacity_actions(&mut decision, 3, &config);
    assert_eq!(decision.needs_more_workers, 0);

    let active = core::SubagentSnapshot {
        active_unit_ids: HashSet::from(["worker".to_owned()]),
        active_models: HashMap::from([("worker".to_owned(), "gpt".to_owned())]),
    };
    let disabled_reassessment = SchedulerConfig {
        reevaluate_on_completion: false,
        ..config.clone()
    };
    policy::apply_reassessment_actions(
        &mut decision,
        &active,
        &parallel_request,
        &disabled_reassessment,
        true,
    );

    decision.completed_recently = 1;
    policy::apply_reassessment_actions(&mut decision, &active, &parallel_request, &config, false);
    policy::apply_floor_action(&mut decision, &parallel_request, &config);
    policy::apply_diversity_action(&mut decision, &parallel_request, &config);
    policy::apply_reuse_actions(
        &mut decision,
        &parallel_request,
        &SchedulerConfig {
            allow_reuse: false,
            cleanup_on_exit: false,
            ..config.clone()
        },
    );
    assert!(decision.needs_model_diversity);
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("immediately"))
    );
    assert!(
        !decision
            .actions
            .iter()
            .any(|action| action.contains("reclaim"))
    );

    policy::apply_floor_action(&mut decision, &parallel_request, &config);
    decision.active_model_families = 2;
    policy::apply_diversity_action(&mut decision, &parallel_request, &config);
    policy::clear_empty_decision(&mut decision, &active);
    assert!(!decision.actions.is_empty());
}

#[test]
fn persistence_prunes_stale_entries_and_bounds_the_cache() {
    let now = Instant::now();
    let stale = core::LiveThreadState {
        last_seen: now - Duration::from_secs(3_601),
        last_reassessed: now,
        active_units: HashSet::new(),
    };
    let mut inner = Inner {
        config: SchedulerConfig::default(),
        threads: HashMap::from([("stale".to_owned(), stale)]),
    };
    policy::persist_thread(
        &mut inner,
        "current".to_owned(),
        now,
        false,
        now - Duration::from_secs(1),
        HashSet::from(["worker".to_owned()]),
    );
    assert_eq!(inner.threads.len(), 1);
    assert_eq!(
        inner.threads["current"].last_reassessed,
        now - Duration::from_secs(1)
    );

    for index in 0..1_024 {
        inner.threads.insert(
            format!("worker-{index}"),
            core::LiveThreadState {
                last_seen: now,
                last_reassessed: now,
                active_units: HashSet::new(),
            },
        );
    }
    policy::persist_thread(
        &mut inner,
        "overflow".to_owned(),
        now,
        true,
        now,
        HashSet::new(),
    );
    assert!(inner.threads.is_empty());
}

#[test]
fn estimates_structured_work_and_handles_all_list_markers() {
    let config = SchedulerConfig {
        max_parallel_workers: 5,
        ..SchedulerConfig::default()
    };
    let snapshot = core::SubagentSnapshot {
        active_unit_ids: HashSet::new(),
        active_models: HashMap::from([
            ("first".to_owned(), "gpt".to_owned()),
            ("second".to_owned(), "grok".to_owned()),
            ("third".to_owned(), "claude".to_owned()),
            ("fourth".to_owned(), "codex".to_owned()),
        ]),
    };
    let structured = messages(&[serde_json::json!({
        "role": "user",
        "content": "  - dash\n* star\n・ dot\n1. numbered\n2) ignored\n9 ignored\nx"
    })]);

    assert_eq!(
        policy::estimate_target_workers(&snapshot, &structured, &config),
        5
    );
    let plain = messages(&[serde_json::json!({"role": "user", "content": "1) no\n9 no\nx"})]);
    assert_eq!(
        policy::estimate_target_workers(&core::SubagentSnapshot::default(), &plain, &config),
        0
    );
}

#[test]
fn parses_scheduler_environment_values_and_rejects_invalid_inputs() {
    let _lock = env_test_lock();
    const USIZE: &str = "CLAUDEX_TEST_PARSE_USIZE";
    const U64: &str = "CLAUDEX_TEST_PARSE_U64";
    const BOOL: &str = "CLAUDEX_TEST_PARSE_BOOL";
    unsafe {
        std::env::remove_var(USIZE);
        std::env::remove_var(U64);
        std::env::remove_var(BOOL);
    }
    assert_eq!(env::parse_usize_env(USIZE), None);
    assert_eq!(env::parse_u64_env(U64), None);
    assert_eq!(env::parse_bool_env(BOOL), None);

    unsafe {
        std::env::set_var(USIZE, "17");
        std::env::set_var(U64, u64::MAX.to_string());
    }
    assert_eq!(env::parse_usize_env(USIZE), Some(17));
    assert_eq!(env::parse_u64_env(U64), Some(u64::MAX));

    for (value, expected) in [
        ("1", true),
        ("True", true),
        ("YES", true),
        ("on", true),
        ("0", false),
        ("False", false),
        ("no", false),
        ("OFF", false),
    ] {
        unsafe { std::env::set_var(BOOL, value) };
        assert_eq!(env::parse_bool_env(BOOL), Some(expected));
    }

    unsafe {
        std::env::set_var(USIZE, "-1");
        std::env::set_var(U64, "invalid");
        std::env::set_var(BOOL, "maybe");
    }
    assert_eq!(env::parse_usize_env(USIZE), None);
    assert_eq!(env::parse_u64_env(U64), None);
    assert_eq!(env::parse_bool_env(BOOL), None);
    unsafe {
        std::env::remove_var(USIZE);
        std::env::remove_var(U64);
        std::env::remove_var(BOOL);
    }
}
