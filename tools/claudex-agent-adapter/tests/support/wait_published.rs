use std::{
    path::Path,
    process::Child,
    thread,
    time::{Duration, Instant},
};

pub const POLL: Duration = Duration::from_millis(10);

pub fn publish_budget() -> Duration {
    if cfg!(coverage_nightly) {
        Duration::from_secs(45)
    } else {
        Duration::from_secs(5)
    }
}

pub fn readable(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn poll_published<T>(
    path: &Path,
    peer: Option<&mut Child>,
    what: &str,
    deadline: Instant,
    parse: impl Fn(&Path) -> Option<T>,
) -> Option<T> {
    if let Some(value) = parse(path) {
        return Some(value);
    }
    fail_if_peer_exited(peer, what);
    if Instant::now() >= deadline {
        panic!("{what}");
    }
    None
}

fn fail_if_peer_exited(peer: Option<&mut Child>, what: &str) {
    let Some(peer) = peer else {
        return;
    };
    if let Some(status) = peer.try_wait().expect("inspect peer process") {
        panic!("{what}: peer exited before publishing ({status})");
    }
}

pub fn wait_until_published<T>(
    path: &Path,
    mut peer: Option<&mut Child>,
    what: &str,
    parse: impl Fn(&Path) -> Option<T>,
) -> T {
    let deadline = Instant::now() + publish_budget();
    loop {
        if let Some(value) = poll_published(path, peer.as_deref_mut(), what, deadline, &parse) {
            return value;
        }
        thread::sleep(POLL);
    }
}
