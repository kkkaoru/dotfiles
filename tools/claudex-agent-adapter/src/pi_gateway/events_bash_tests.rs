use serde_json::json;

use super::normalize;

#[test]
fn backgrounds_long_timeouts_and_blocking_watchers() {
    assert_eq!(
        normalize("Bash", json!({"command":"cargo test","timeout":120000})),
        json!({"command":"cargo test","timeout":120000,"run_in_background":true})
    );
    assert_eq!(
        normalize("Bash", json!({"command":"echo ready; gh run watch 123"})),
        json!({"command":"echo ready; gh run watch 123","run_in_background":true})
    );
    assert_eq!(
        normalize("Bash", json!({"command":"tail\t-f output.log"})),
        json!({"command":"tail\t-f output.log","run_in_background":true})
    );
    assert_eq!(
        normalize("Bash", json!({"command":"watch date"})),
        json!({"command":"watch date","run_in_background":true})
    );
}

#[test]
fn preserves_short_explicit_background_tmux_and_non_bash_calls() {
    assert_eq!(
        normalize("Bash", json!({"command":"gh status"})),
        json!({"command":"gh status"})
    );
    assert_eq!(
        normalize("Bash", json!({"command":"gh run list"})),
        json!({"command":"gh run list"})
    );
    assert_eq!(
        normalize("Bash", json!({"command":"cargo test","timeout":119999})),
        json!({"command":"cargo test","timeout":119999})
    );
    assert_eq!(
        normalize(
            "Bash",
            json!({"command":"watch date","run_in_background":true})
        ),
        json!({"command":"watch date","run_in_background":true})
    );
    assert_eq!(
        normalize(
            "Bash",
            json!({"command":"tmux new-session -d 'watch date'","timeout":120000})
        ),
        json!({"command":"tmux new-session -d 'watch date'","timeout":120000})
    );
    assert_eq!(
        normalize("Read", json!({"path":"README.md"})),
        json!({"path":"README.md"})
    );
    assert_eq!(normalize("Bash", json!([])), json!([]));
    assert_eq!(
        normalize("Bash", json!({"timeout":120000})),
        json!({"timeout":120000})
    );
}
