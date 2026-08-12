use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt as _;

struct RequestFixture {
    root: tempfile::TempDir,
    cache: PathBuf,
}

impl RequestFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("usage-routing.json");
        Self { root, cache }
    }

    fn request(&self) -> SpawnRequest<'_> {
        SpawnRequest {
            cache_path: &self.cache,
            home: self.root.path(),
            config_path: Path::new("/providers.json"),
            disabled_path: Path::new("/disabled.json"),
            codexbar_program: "/bin/echo",
            curl_program: "/usr/bin/curl",
            configuration_key: "policy-key",
        }
    }
}

#[test]
fn fresh_skips_and_cold_claims_nonblocking_singleflight() {
    let fixture = RequestFixture::new();
    let calls = Cell::new(0);
    assert!(!schedule_with(&fixture.request(), true, |_, _, _| Ok(())).unwrap());
    assert!(
        schedule_with(&fixture.request(), false, |_, _, _| {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap()
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn lock_is_released_by_kernel_when_owner_file_drops() {
    let root = tempfile::tempdir().unwrap();
    let path = lock_path(&root.path().join("usage-routing.json"));
    let first = private_lock_file(&path).unwrap();
    assert!(lock_file(&first, true).unwrap());
    let second = private_lock_file(&path).unwrap();
    assert!(!lock_file(&second, true).unwrap());
    drop(first);
    assert!(lock_file(&second, true).unwrap());
}

#[test]
fn lock_path_rejects_symlink_directory_and_permissive_file() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::write(&target, "unchanged").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let symlink = root.path().join("link");
    std::os::unix::fs::symlink(&target, &symlink).unwrap();
    assert!(private_lock_file(&symlink).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");

    let directory = root.path().join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(private_lock_file(&directory).is_err());

    let permissive = root.path().join("permissive");
    fs::write(&permissive, "").unwrap();
    fs::set_permissions(&permissive, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(private_lock_file(&permissive).is_err());
}

#[test]
fn publication_generation_is_monotonic_even_when_time_moves_backward() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("usage-routing.json");
    util::write_routing_cache(&cache, &serde_json::json!({}), 100.0, "key", 7).unwrap();
    assert!(publish_sync(&cache, &serde_json::json!({}), 50.0, "key").unwrap());
    assert_eq!(util::cache_generation(&cache), Some(8));
}

#[test]
fn busy_sync_writer_leaves_existing_cache_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("usage-routing.json");
    util::write_routing_cache(&cache, &serde_json::json!({"old":true}), 1.0, "key", 4).unwrap();
    let before = fs::read(&cache).unwrap();
    let lock = private_lock_file(&lock_path(&cache)).unwrap();
    assert!(lock_file(&lock, true).unwrap());
    assert!(!publish_sync(&cache, &serde_json::json!({"new":true}), 2.0, "key").unwrap());
    assert_eq!(fs::read(cache).unwrap(), before);
}

#[test]
fn replaced_lock_path_cannot_publish_through_the_old_inode() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("usage-routing.json");
    let path = lock_path(&cache);
    let old = private_lock_file(&path).unwrap();
    assert!(lock_file(&old, true).unwrap());
    fs::rename(&path, root.path().join("old-lock")).unwrap();
    let replacement = private_lock_file(&path).unwrap();
    assert!(lock_file(&replacement, true).unwrap());
    assert!(!lock_path_matches(&old, &path).unwrap());
}
