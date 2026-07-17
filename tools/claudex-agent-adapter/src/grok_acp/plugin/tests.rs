use std::ffi::OsStr;

use super::{EFFORTS, ROUTING_INSTRUCTIONS, prepare, prepare_with, profile, write_if_changed};

#[test]
fn profiles_define_every_routable_effort() {
    for effort in EFFORTS {
        let profile = profile(effort);
        assert!(profile.contains(&format!("effort: {effort}")));
        assert!(ROUTING_INSTRUCTIONS.contains(&format!("claudex-{effort}")));
    }
}

#[test]
fn custom_program_does_not_receive_builtin_plugin() {
    assert_eq!(prepare(OsStr::new("grok-acp-mock")).unwrap(), None);

    let plugin = tempfile::tempdir().unwrap();
    assert_eq!(
        prepare_with(OsStr::new("grok"), Some(plugin.path().to_owned()), None).unwrap(),
        Some(plugin.path().to_owned())
    );
    assert!(prepare_with(OsStr::new("grok"), None, None).is_err());
}

#[test]
fn prepares_and_reuses_every_builtin_effort_profile() {
    let home = tempfile::tempdir().unwrap();
    let plugin = prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned()))
        .unwrap()
        .unwrap();
    assert!(plugin.join("agents/claudex-medium.md").is_file());
    assert_eq!(
        prepare_with(OsStr::new("grok"), None, Some(home.path().to_owned())).unwrap(),
        Some(plugin)
    );
}

#[test]
fn writes_profiles_only_when_needed_and_reports_write_failures() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile.md");
    write_if_changed(profile.clone(), "content").unwrap();
    write_if_changed(profile, "content").unwrap();

    let error = write_if_changed(root.path().join("missing/profile.md"), "content").unwrap_err();
    assert!(error.to_string().contains("write"));
}
