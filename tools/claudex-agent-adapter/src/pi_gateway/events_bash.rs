use serde_json::{Map, Value, json};

const LONG_TIMEOUT_MILLISECONDS: u64 = 120_000;

pub(super) fn normalize(name: &str, arguments: Value) -> Value {
    if !name.eq_ignore_ascii_case("Bash") {
        return arguments;
    }
    let Value::Object(mut object) = arguments else {
        return arguments;
    };
    if should_background(&object) {
        object.insert("run_in_background".to_owned(), json!(true));
    }
    Value::Object(object)
}

fn should_background(arguments: &Map<String, Value>) -> bool {
    if arguments.get("run_in_background").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let Some(command) = arguments.get("command").and_then(Value::as_str) else {
        return false;
    };
    if command.split_whitespace().any(|word| word == "tmux") {
        return false;
    }
    has_long_timeout(arguments) || has_watcher(command)
}

fn has_long_timeout(arguments: &Map<String, Value>) -> bool {
    arguments
        .get("timeout")
        .and_then(Value::as_u64)
        .is_some_and(|timeout| timeout >= LONG_TIMEOUT_MILLISECONDS)
}

fn has_watcher(command: &str) -> bool {
    command.split(['\n', ';', '&', '|']).any(segment_is_watcher)
}

fn segment_is_watcher(segment: &str) -> bool {
    let mut words = segment.split_whitespace();
    match words.next() {
        Some("watch") => true,
        Some("tail") => words.next() == Some("-f"),
        Some("gh") => words.next() == Some("run") && words.next() == Some("watch"),
        _ => false,
    }
}

#[cfg(test)]
#[path = "events_bash_tests.rs"]
mod tests;
