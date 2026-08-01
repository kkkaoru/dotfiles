use std::net::SocketAddr;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;

#[test]
fn validates_generation_names_and_manifest_arguments() {
    assert!(safe_component("v1-build.config-1"));
    for value in ["", "contains/slash", "contains space", "contains\nnewline"] {
        assert!(!safe_component(value));
    }
    assert!(generation_from_path(Path::new("manifest.valid-name.json")).is_some());
    for path in [
        Path::new("manifest.json"),
        Path::new("wrong.valid-name.json"),
        Path::new("manifest.invalid/name.json"),
        Path::new("manifest.bad name.json"),
    ] {
        assert!(generation_from_path(path).is_none());
    }

    let listen: SocketAddr = "127.0.0.1:8318".parse().expect("listen");
    let mut manifest = RecoveryManifest {
        generation: "valid".to_owned(),
        protocol_version: 1,
        build_id: "build".to_owned(),
        listen,
        model: "model".to_owned(),
        arguments: vec![
            "serve".to_owned(),
            "--listen".to_owned(),
            listen.to_string(),
        ],
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
    };
    validate_arguments(&manifest).expect("valid daemon arguments");
    manifest.arguments[0] = "launch".to_owned();
    assert!(validate_arguments(&manifest).is_err());
    manifest.arguments = vec!["serve".to_owned()];
    assert!(validate_arguments(&manifest).is_err());
    manifest.arguments = vec![
        "serve".to_owned(),
        "--listen".to_owned(),
        "127.0.0.1:1".to_owned(),
    ];
    assert!(validate_arguments(&manifest).is_err());
}

#[test]
fn cleanup_prunes_only_unreferenced_private_generations_and_bad_entries() {
    let root = tempfile::tempdir().expect("recovery root");
    let listen: SocketAddr = "127.0.0.1:8318".parse().expect("listen");
    let current_build = root.path().join("build-current");
    let unused_build = root.path().join("build-unused");
    std::fs::create_dir_all(&current_build).expect("current build");
    std::fs::create_dir_all(&unused_build).expect("unused build");
    std::fs::create_dir_all(root.path().join("bad name")).expect("unsafe generation");

    let manifest = RecoveryManifest {
        generation: "current".to_owned(),
        protocol_version: 1,
        build_id: "build-current".to_owned(),
        listen,
        model: "model".to_owned(),
        arguments: vec![
            "serve".to_owned(),
            "--listen".to_owned(),
            listen.to_string(),
        ],
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
    };
    let manifest_path = root.path().join(manifest_file_name("current"));
    publish_manifest(&manifest_path, &manifest).expect("publish current manifest");

    let invalid_path = root.path().join("manifest.invalid.json");
    std::fs::write(&invalid_path, b"not json").expect("invalid manifest");
    #[cfg(unix)]
    std::fs::set_permissions(&invalid_path, std::fs::Permissions::from_mode(0o600))
        .expect("invalid manifest permissions");
    assert!(
        manifest_entry(
            std::fs::read_dir(root.path())
                .unwrap()
                .find(|entry| {
                    entry
                        .as_ref()
                        .map(|entry| entry.path() == invalid_path)
                        .unwrap_or(false)
                })
                .unwrap()
                .unwrap()
        )
        .is_none()
    );
    assert!(manifests(Path::new("/path/that/does/not/exist")).is_empty());

    cleanup(root.path(), listen, "current").expect("cleanup recovery root");
    assert!(current_build.exists());
    assert!(!unused_build.exists());
    assert!(root.path().join("bad name").exists());
}
