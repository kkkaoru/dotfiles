use std::ffi::OsString;

use super::expand_home_with;

#[test]
fn expands_home_only_for_tilde_relative_paths() {
    assert_eq!(
        expand_home_with("catalog.json", Some(OsString::from("/tmp/home"))),
        "catalog.json"
    );
    assert_eq!(
        expand_home_with("~/.codex/catalog.json", Some(OsString::from("/tmp/home"))),
        "/tmp/home/.codex/catalog.json"
    );
    assert_eq!(
        expand_home_with("~/.codex/catalog.json", None),
        "~/.codex/catalog.json"
    );
}
