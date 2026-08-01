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
fn deduplicates_same_prompt_across_fresh_tool_use_ids() {
    let scheduler = ParallelScheduler::for_tests();
    let state = messages(&[
        serde_json::json!({
            "role":"user",
            "content":"Handle this one bounded task.",
        }),
        serde_json::json!({
            "role":"assistant",
            "content":[
                {"type":"tool_use","id":"retry-a","name":"cc_Agent_0","input":{"prompt":"Handle this one bounded task.","claudex_model":"gpt-5.6-sol"}},
                {"type":"tool_use","id":"retry-b","name":"cc_Agent_0","input":{"prompt":" handle   this one bounded task. ","claudex_model":"grok-4.5"}},
            ]
        }),
    ]);

    let decision = scheduler.decision_for_request(&state);

    assert_eq!(decision.active_workers, 1);
    assert_eq!(decision.target_workers, 1);
    assert_eq!(decision.needs_more_workers, 0);
}

#[test]
fn existing_workers_above_scope_count_do_not_inflate_the_target() {
    let scheduler = ParallelScheduler::for_tests();
    let request = messages(&[
        serde_json::json!({
            "role":"user",
            "content":"Complete the independent scopes.\n- inspect transport behavior\n- inspect cache behavior"
        }),
        serde_json::json!({
            "role":"assistant",
            "content":[
                {"type":"tool_use","id":"lane-a","name":"cc_Agent_0","input":{"prompt":"scope a","claudex_model":"model-a"}},
                {"type":"tool_use","id":"lane-b","name":"cc_Agent_0","input":{"prompt":"scope b","claudex_model":"model-b"}},
                {"type":"tool_use","id":"lane-c","name":"cc_Agent_0","input":{"prompt":"scope c","claudex_model":"model-c"}},
                {"type":"tool_use","id":"lane-d","name":"cc_Agent_0","input":{"prompt":"scope d","claudex_model":"model-d"}},
                {"type":"tool_use","id":"lane-e","name":"cc_Agent_0","input":{"prompt":"scope e","claudex_model":"model-e"}}
            ]
        }),
    ]);

    let decision = scheduler.decision_for_request(&request);

    assert_eq!(decision.active_workers, 5);
    assert_eq!(decision.target_workers, 2);
    assert_eq!(decision.needs_more_workers, 0);
    assert!(
        decision
            .actions
            .iter()
            .all(|action| !action.contains("Launch at least"))
    );
}

#[test]
fn async_launch_acknowledgement_keeps_native_workers_active_until_notification() {
    let scheduler = ParallelScheduler::for_tests();
    let launch = serde_json::json!({
        "role":"assistant",
        "content":[
            {"type":"tool_use","id":"scope-a","name":"cc_Agent_0","input":{"prompt":"scope a","claudex_model":"gpt-5.6-sol"}},
            {"type":"tool_use","id":"scope-b","name":"cc_Agent_0","input":{"prompt":"scope b","claudex_model":"grok-4.5"}}
        ]
    });
    let acknowledgement = serde_json::json!({
        "role":"user",
        "content":[
            {
                "type":"tool_result", "tool_use_id":"scope-a",
                "content":[{"type":"text", "text":"Async agent launched successfully.\nThe agent is working in the background."}]
            },
            {
                "type":"tool_result", "tool_use_id":"scope-b",
                "content":[{"type":"text", "text":"Async agent launched successfully.\nThe agent is working in the background."}]
            }
        ]
    });
    let running = messages(&[
        serde_json::json!({"role":"user","content":"Run these scopes.\n- scope a\n- scope b"}),
        launch.clone(),
        acknowledgement.clone(),
    ]);
    let running_decision = scheduler.decision_for_request(&running);
    assert_eq!(running_decision.active_workers, 2);
    assert_eq!(running_decision.completed_recently, 0);
    assert_eq!(running_decision.needs_more_workers, 0);

    let completed = messages(&[
        serde_json::json!({"role":"user","content":"Run these scopes.\n- scope a\n- scope b"}),
        launch,
        acknowledgement,
        serde_json::json!({
            "role":"user",
            "content":"<task-notification>\n<task-id>native-a</task-id>\n<tool-use-id>scope-a</tool-use-id>\n<status>completed</status>\n<result>done</result>\n</task-notification>"
        }),
    ]);
    let completed_decision = scheduler.decision_for_request(&completed);
    assert_eq!(completed_decision.active_workers, 1);
    assert_eq!(completed_decision.completed_recently, 1);
    assert_eq!(completed_decision.needs_more_workers, 1);
}

#[test]
fn partial_and_duplicate_async_acknowledgements_complete_only_the_reported_scope() {
    let launch = serde_json::json!({
        "role":"assistant",
        "content":[
            {"type":"tool_use","id":"scope-a","name":"cc_Agent_0","input":{"prompt":"scope a","claudex_model":"gpt-5.6-sol"}},
            {"type":"tool_use","id":"scope-b","name":"cc_Agent_0","input":{"prompt":"scope b","claudex_model":"grok-4.5"}}
        ]
    });
    let result = serde_json::json!({
        "type":"tool_result", "tool_use_id":"scope-a",
        "content":"Async agent launched successfully.\nThe agent is working in the background."
    });
    for acknowledgement in [
        serde_json::json!({"role":"user", "content":[&result]}),
        serde_json::json!({"role":"user", "content":[&result, &result]}),
    ] {
        let snapshot = core::analyze_subagent_work(&[launch.clone(), acknowledgement]);
        assert_eq!(snapshot.active_count(), 1);
        assert!(snapshot.active_unit_ids.contains("scope:scope b"));
    }
}

#[test]
fn legacy_batch_activity_does_not_inflate_a_single_scope_target() {
    let scheduler = ParallelScheduler::for_tests();
    let state = messages(&[
        serde_json::json!({
            "role":"user",
            "content":"Run the explicitly requested batch.",
        }),
        serde_json::json!({
            "role":"assistant",
            "content":[{
                "type":"tool_use",
                "id":"batch-explicit",
                "name":"cc_Agent_batch_0",
                "input":{"tasks":[
                    {"prompt":"same scope","claudex_model":"gpt-5.6-sol"},
                    {"prompt":"same scope","claudex_model":"grok-4.5"},
                    {"prompt":"same scope","claudex_model":"gpt-5.6-sol"}
                ]}
            }]
        }),
    ]);

    let decision = scheduler.decision_for_request(&state);

    assert_eq!(decision.active_workers, 3);
    assert_eq!(decision.target_workers, 1);
    assert_eq!(decision.needs_more_workers, 0);
}

#[test]
fn completing_one_duplicate_scope_does_not_trigger_a_relaunch() {
    let scheduler = ParallelScheduler::for_tests();
    let first = messages(&[
        serde_json::json!({"role":"user","content":"Handle one bounded task."}),
        serde_json::json!({"role":"assistant","content":[
            {"type":"tool_use","id":"same-a","name":"cc_Agent_0","input":{"prompt":"Handle one bounded task.","claudex_model":"gpt-5.6-sol"}},
            {"type":"tool_use","id":"same-b","name":"cc_Agent_0","input":{"prompt":"Handle one bounded task.","claudex_model":"gpt-5.6-sol"}}
        ]}),
    ]);
    let after_one_completion = messages(&[
        serde_json::json!({"role":"user","content":"Handle one bounded task."}),
        serde_json::json!({"role":"assistant","content":[
            {"type":"tool_result","tool_use_id":"same-a","content":"done"},
            {"type":"tool_use","id":"same-b","name":"cc_Agent_0","input":{"prompt":"Handle one bounded task.","claudex_model":"gpt-5.6-sol"}}
        ]}),
    ]);

    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&after_one_completion);

    assert_eq!(decision.active_workers, 1);
    assert_eq!(decision.completed_recently, 0);
    assert_eq!(decision.needs_more_workers, 0);
}

#[test]
fn unknown_task_result_does_not_restart_other_running_scopes() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 2,
        max_parallel_workers: 2,
        active_floor: 2,
        ..SchedulerConfig::default()
    });
    let first = messages(&[
        serde_json::json!({"role":"user","content":"Run these independent scopes in parallel."}),
        serde_json::json!({"role":"assistant","content":[
            {"type":"tool_use","id":"running-a","name":"cc_Agent_0","input":{"prompt":"scope a","claudex_model":"gpt-5.6-sol"}},
            {"type":"tool_use","id":"running-b","name":"cc_Agent_0","input":{"prompt":"scope b","claudex_model":"gpt-5.6-sol"}}
        ]}),
    ]);
    let stale_result = messages(&[
        serde_json::json!({"role":"user","content":"Run these independent scopes in parallel."}),
        serde_json::json!({"role":"assistant","content":[
            {"type":"tool_result","tool_use_id":"unknown-stale-task","content":"Error: No task found with ID: unknown-stale-task"},
            {"type":"tool_use","id":"running-a","name":"cc_Agent_0","input":{"prompt":"scope a","claudex_model":"gpt-5.6-sol"}},
            {"type":"tool_use","id":"running-b","name":"cc_Agent_0","input":{"prompt":"scope b","claudex_model":"gpt-5.6-sol"}}
        ]}),
    ]);

    let _ = scheduler.decision_for_request(&first);
    let decision = scheduler.decision_for_request(&stale_result);

    assert_eq!(decision.active_workers, 2);
    assert_eq!(decision.completed_recently, 0);
    assert_eq!(decision.needs_more_workers, 0);
    assert!(
        !decision
            .actions
            .iter()
            .any(|action| action.contains("Launch at least"))
    );
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
fn explicit_parallel_request_uses_inferred_scope_count_without_list_markers() {
    let scheduler = ParallelScheduler::for_tests();
    let request = messages(&[
        serde_json::json!({
            "role": "user",
            "content": "架空組織 Example Labs を複数のSubAgentで並列調査してください",
        }),
        serde_json::json!({
            "role": "assistant",
            "content": [tool_use("company", "cc_Agent_0", "gpt-5.6-sol")],
        }),
    ]);

    let decision = scheduler.decision_for_request(&request);

    assert_eq!(decision.target_workers, 2);
    assert_eq!(decision.needs_more_workers, 1);
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
            "content": "架空組織 Example Labs を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向",
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
            "content": "架空組織 Example Labs を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向",
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
            "content": "架空組織 Example Labs を調査してください。\n- 会社概要\n- 資金調達\n- 競合と市場動向",
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
fn policy_helpers_cover_live_reuse_and_single_scope_guidance_boundaries() {
    let config = SchedulerConfig::default();
    let parallel_request = messages(&[serde_json::json!({
        "role": "user",
        "content": "調査を分担して並列で実行してください。\n- 概要\n- リスク",
    })]);
    let single_request = messages(&[serde_json::json!({
        "role": "user",
        "content": "この1件を確認してください",
    })]);

    let mut active = SchedulerDecision::no_action();
    active.active_workers = 1;
    policy::apply_reuse_actions(&mut active, &parallel_request, &config);
    assert!(
        active
            .actions
            .iter()
            .any(|action| action.contains("reusing"))
    );

    let mut completed = active.clone();
    completed.completed_recently = 1;
    policy::apply_reuse_actions(&mut completed, &parallel_request, &config);
    assert!(
        completed
            .actions
            .iter()
            .any(|action| action.contains("completion-aware"))
    );

    let mut pending = SchedulerDecision::no_action();
    pending.needs_more_workers = 1;
    policy::clear_empty_decision(&mut pending, &core::SubagentSnapshot::default());
    assert_eq!(pending.needs_more_workers, 1);

    let mut crowded = SchedulerDecision::no_action();
    crowded.active_workers = 2;
    assert!(policy::scope_guidance(&single_request, &crowded).contains("stop duplicate"));
    assert!(policy::scope_guidance(&single_request, &active).contains("exactly one"));
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
        4
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

#[test]
fn covers_malformed_work_units_and_policy_boundaries() {
    let snapshot = core::analyze_subagent_work(&[
        serde_json::json!({"role":"assistant", "content":null}),
        serde_json::json!({
            "role":"assistant",
            "content":[
                {"type":"tool_use"},
                {"type":"tool_use", "id":"missing-input", "name":"cc_Agent_0", "input":null},
                {"type":"tool_use", "id":"batch-missing", "name":"cc_Agent_batch_0", "input":{}},
                {"type":"tool_use", "id":"batch", "name":"cc_Agent_batch_0", "input":{"tasks":[
                    null,
                    {"subagent_type":"custom-advisor", "claudex_model":"advisor"},
                    {"claudex_model":"worker"}
                ]}},
                {"type":"tool_result"},
                {"type":"unknown"}
            ]
        }),
        serde_json::json!({
            "role":"user",
            "content":[
                {"type":"text", "text":"<task-notification><status>unknown</status></task-notification>"},
                {"type":"text", "text":"<task-notification><status>completed</status><tool-use-id></tool-use-id></task-notification>"}
            ]
        }),
    ]);
    assert_eq!(snapshot.active_count(), 1);
    assert!(snapshot.active_models.contains_key("batch:2"));

    let no_user = messages(&[serde_json::json!({"role":"assistant", "content":[]} )]);
    assert_eq!(policy::independent_scope_count(&no_user), 2);
    let single = messages(&[serde_json::json!({"role":"user", "content":"exactly one worker"})]);
    assert_eq!(policy::independent_scope_count(&single), 1);
    let explicit = messages(&[serde_json::json!({"role":"user", "content":"- one\n* two"})]);
    assert_eq!(policy::independent_scope_count(&explicit), 2);
    let parallel =
        messages(&[serde_json::json!({"role":"user", "content":"compare these in parallel"})]);
    assert_eq!(policy::independent_scope_count(&parallel), 2);
    let plain = messages(&[serde_json::json!({"role":"user", "content":"one bounded task"})]);

    let mut decision = SchedulerDecision::no_action();
    decision.active_workers = 2;
    decision.active_model_families = 1;
    decision.completed_recently = 1;
    let config = SchedulerConfig {
        allow_reuse: false,
        cleanup_on_exit: false,
        ..SchedulerConfig::default()
    };
    policy::apply_diversity_action(&mut decision, &parallel, &config);
    policy::apply_reuse_actions(&mut decision, &parallel, &config);
    assert!(decision.needs_model_diversity);
    assert!(
        decision
            .actions
            .iter()
            .all(|action| !action.contains("Prefer reusing"))
    );
    assert!(
        decision
            .actions
            .iter()
            .any(|action| action.contains("After each completion"))
    );
    policy::apply_reuse_actions(&mut decision, &plain, &config);

    let mut empty = SchedulerDecision::no_action();
    policy::clear_empty_decision(&mut empty, &core::SubagentSnapshot::default());
    assert!(empty.actions.is_empty());
}

#[test]
fn parses_all_scheduler_environment_overrides() {
    let _lock = env_test_lock();
    clear_scheduler_env();
    unsafe {
        std::env::set_var(SUBAGENT_MAX_PARALLEL_ENV, "8");
        std::env::set_var(SUBAGENT_MIN_PARALLEL_ENV, "4");
        std::env::set_var(SUBAGENT_ACTIVE_FLOOR_ENV, "3");
        std::env::set_var(SUBAGENT_REEVALUATE_ON_COMPLETION_ENV, "false");
        std::env::set_var(SUBAGENT_REASSESS_INTERVAL_SECONDS_ENV, "17");
        std::env::set_var(SUBAGENT_MIN_MODEL_FAMILIES_ENV, "3");
        std::env::set_var(SUBAGENT_REUSE_ENV, "false");
        std::env::set_var(SUBAGENT_CLEANUP_ON_EXIT_ENV, "false");
    }
    let config = SchedulerConfig::parse();
    assert_eq!(config.max_parallel_workers, 8);
    assert_eq!(config.min_parallel_workers, 4);
    assert_eq!(config.active_floor, 3);
    assert!(!config.reevaluate_on_completion);
    assert_eq!(config.reassess_interval, Duration::from_secs(17));
    assert_eq!(config.min_model_families, 3);
    assert!(!config.allow_reuse);
    assert!(!config.cleanup_on_exit);
    clear_scheduler_env();
}

#[test]
fn normalizes_scheduler_scopes_and_task_notifications_without_false_duplicates() {
    let messages = vec![
        serde_json::json!({
            "role":"assistant",
            "content":[{"type":"tool_use", "id":"scope-one", "name":"Agent", "input":{
                "claudex_model":"worker-a",
                "prompt":"Research this\nclaudex_launch_id: hidden\nclaudex_model: worker-a <claudex-agent-id>correlation</claudex-agent-id>"
            }}]
        }),
        serde_json::json!({
            "role":"assistant",
            "content":[{"type":"tool_use", "id":"scope-two", "name":"Agent", "input":{
                "claudex_model":"worker-b",
                "prompt":"Research this <claudex-agent-id>other</claudex-agent-id>"
            }}]
        }),
        serde_json::json!({
            "role":"user",
            "content":"<task-notification><status>failed</status><tool-use-id>scope-two</tool-use-id></task-notification>"
        }),
    ];
    let snapshot = core::analyze_subagent_work(&messages);
    assert_eq!(snapshot.active_count(), 1);
    assert!(snapshot.active_unit_ids.contains("scope:research this"));
    assert_eq!(snapshot.active_model_families(), 1);

    let unclosed = vec![serde_json::json!({
        "role":"assistant",
        "content":[{"type":"tool_use", "id":"unclosed", "name":"Agent", "input":{
            "claudex_model":"worker-c",
            "prompt":"scope <claudex-agent-id>unfinished"
        }}]
    })];
    let snapshot = core::analyze_subagent_work(&unclosed);
    assert_eq!(snapshot.active_count(), 1);
    assert!(
        snapshot
            .active_unit_ids
            .iter()
            .any(|unit| unit.starts_with("scope:scope "))
    );
}
