use super::*;
use std::{fs, path::PathBuf, time::SystemTime};

#[test]
fn writes_and_detects_a_cachedir_tag() {
    let root = tempfile::tempdir().expect("tag fixture");
    let dir = root.path().join("target");
    assert!(!has_cachedir_tag(&dir));
    write_cachedir_tag(&dir).expect("write tag");
    assert!(has_cachedir_tag(&dir));
    let contents = fs::read_to_string(dir.join(CACHEDIR_TAG_NAME)).expect("read tag");
    assert!(contents.contains("8a477f597d28d172789f06886806bc55"));
}

#[test]
fn writing_a_cachedir_tag_reports_io_errors() {
    let root = tempfile::tempdir().expect("tag error fixture");
    let file = root.path().join("not-a-directory");
    fs::write(&file, b"file").expect("tag error file");
    let create_error = write_cachedir_tag(&file).expect_err("file parent must fail");
    assert!(
        create_error.to_string().contains("create"),
        "{create_error}"
    );

    let target = root.path().join("target");
    fs::create_dir(&target).expect("tag target");
    fs::create_dir(target.join(CACHEDIR_TAG_NAME)).expect("tag path directory");
    let write_error = write_cachedir_tag(&target).expect_err("tag path directory must fail");
    assert!(
        write_error.to_string().contains("write CACHEDIR.TAG"),
        "{write_error}"
    );
}

#[test]
fn prune_cache_deletes_only_tagged_descendants() {
    let root = tempfile::tempdir().expect("prune fixture");
    let tagged = root.path().join("tagged");
    let untagged = root.path().join("untagged");
    write_cachedir_tag(&tagged).expect("tag");
    fs::create_dir(&untagged).expect("untagged");
    fs::write(untagged.join("keep"), "data").expect("keep");
    age_path(
        &tagged,
        TAGGED_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );
    let removed = prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune");
    assert_eq!(removed, 1);
    assert!(!tagged.exists());
    assert!(untagged.join("keep").is_file());
}

#[test]
fn prune_cache_keeps_recent_tagged_dirs_and_live_coverage() {
    let root = tempfile::tempdir().expect("recent fixture");
    let recent = root.path().join("recent-target");
    write_cachedir_tag(&recent).expect("tag recent");
    let live = root.path().join(format!("llvm-cov-{}", std::process::id()));
    write_cachedir_tag(&live).expect("tag live");
    age_path(
        &live,
        TAGGED_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );
    let removed = prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune");
    assert_eq!(removed, 0);
    assert!(recent.is_dir());
    assert!(live.is_dir());
}

#[test]
fn prune_cache_keeps_the_newest_failed_coverage_targets() {
    let root = tempfile::tempdir().expect("coverage keep fixture");
    let mut dirs = Vec::new();
    for name in ["llvm-cov-11", "llvm-cov-12", "llvm-cov-13"] {
        let path = root.path().join(name);
        write_cachedir_tag(&path).expect("tag");
        dirs.push(path);
    }
    age_path(&dirs[0], std::time::Duration::from_secs(30));
    age_path(&dirs[1], std::time::Duration::from_secs(20));
    age_path(&dirs[2], std::time::Duration::from_secs(10));
    let removed = prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune");
    assert_eq!(removed, 1);
    assert!(!dirs[0].exists());
    assert!(dirs[1].is_dir());
    assert!(dirs[2].is_dir());
}

#[cfg(unix)]
#[test]
fn prune_cache_exercises_expired_coverage_and_sibling_filters() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("coverage filter fixture");
    let stale = root.path().join("llvm-cov-expired");
    let live = root.path().join(format!("llvm-cov-{}", std::process::id()));
    let notes = root.path().join("notes");
    let dangling = root.path().join("dangling-sibling");

    write_cachedir_tag(&stale).expect("tag stale coverage");
    fs::create_dir(&live).expect("live untagged sibling");
    fs::write(&notes, b"not a coverage target").expect("non-coverage sibling");
    symlink(root.path().join("missing-target"), &dangling).expect("dangling sibling");
    age_path(
        &stale,
        COVERAGE_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );

    let removed = prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune");
    assert_eq!(removed, 1);
    assert!(!stale.exists());
    assert!(live.is_dir());
    assert!(notes.is_file());
    assert!(dangling.is_symlink());
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn prune_cache_skips_non_utf8_and_directory_symlink_entries() {
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::symlink},
    };

    let root = tempfile::tempdir().expect("prune guard fixture");
    let outside = tempfile::tempdir().expect("symlink target fixture");
    let non_utf8 = root
        .path()
        .join(OsString::from_vec(b"llvm-cov-\xff".to_vec()));
    let failed = root.path().join("llvm-cov-failed");
    let external_stale = outside.path().join("external-stale");
    let real_dir = root.path().join("real-dir");
    let directory_link = root.path().join("directory-link");

    write_cachedir_tag(&non_utf8).expect("tag non-UTF-8 target");
    write_cachedir_tag(&failed).expect("tag failed target");
    write_cachedir_tag(&external_stale).expect("tag external target");
    fs::create_dir(&real_dir).expect("real directory");
    symlink(outside.path(), &directory_link).expect("directory symlink");
    age_path(
        &non_utf8,
        TAGGED_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );
    age_path(
        &external_stale,
        TAGGED_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );

    let removed = prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune");
    assert_eq!(removed, 1);
    assert!(!non_utf8.exists());
    assert!(failed.is_dir());
    assert!(external_stale.is_dir());
    assert!(directory_link.is_symlink());
}

#[test]
fn prune_cache_reports_walk_errors_for_a_non_directory_root() {
    let root = tempfile::tempdir().expect("walk error fixture");
    let file = root.path().join("not-a-directory");
    fs::write(&file, b"not a directory").expect("walk error file");

    let error = prune_tagged_dirs(&file, SystemTime::now()).expect_err("file root must fail");
    assert!(error.to_string().contains("walk"), "{error}");
}

#[test]
fn prune_cache_does_not_remove_a_tagged_walk_root() {
    let root = tempfile::tempdir().expect("tagged walk root");
    write_cachedir_tag(root.path()).expect("tag walk root");

    assert_eq!(
        prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune"),
        0
    );
    assert!(root.path().is_dir());
}

#[test]
fn disk_probe_reports_space_and_rejects_impossible_minimums() {
    let root = tempfile::tempdir().expect("disk fixture");
    let missing = root.path().join("missing/child");
    assert!(disk::available_bytes(&missing).expect("ancestor space") > 0);
    require_disk_free(root.path(), 0).expect("zero minimum");
    let error = require_disk_free(root.path(), u64::MAX).expect_err("impossible minimum");
    assert!(error.to_string().contains("bytes free"), "{error}");
}

#[cfg(unix)]
#[test]
fn disk_probe_reports_statvfs_and_ancestor_errors() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let empty_path_error = disk::available_bytes(std::path::Path::new(""))
        .expect_err("statvfs must reject an empty path");
    assert!(
        empty_path_error.to_string().contains("statvfs"),
        "{empty_path_error}"
    );

    let nul_path = PathBuf::from(OsString::from_vec(b"invalid\0path".to_vec()));
    let nul_path_error = disk::available_bytes(&nul_path).expect_err("NUL path must fail");
    assert!(
        nul_path_error.to_string().contains("statvfs"),
        "{nul_path_error}"
    );

    let free_error = require_disk_free(std::path::Path::new(""), 0)
        .expect_err("disk requirement must report statvfs errors");
    assert!(free_error.to_string().contains("statvfs"), "{free_error}");
    let coverage_error = require_coverage_disk(std::path::Path::new(""))
        .expect_err("coverage requirement must report statvfs errors");
    assert!(
        coverage_error.to_string().contains("need at least"),
        "{coverage_error}"
    );

    let root = tempfile::tempdir().expect("coverage error fixture");
    let file = root.path().join("not-a-directory");
    fs::write(&file, b"file").expect("coverage error file");
    let prepare_error = prepare_coverage_target(&file)
        .expect_err("coverage target creation must reject a file path");
    assert!(
        prepare_error.to_string().contains("create coverage target"),
        "{prepare_error}"
    );
}

#[test]
fn default_prune_cache_handles_a_missing_home() {
    const CHILD_MARKER: &str = "CLAUDEX_CACHE_HYGIENE_NO_HOME";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let error = default_prune_root().expect_err("HOME must be required");
        assert!(error.to_string().contains("HOME is required"), "{error}");
        prune_default_tagged_cache();
        return;
    }

    let status =
        std::process::Command::new(std::env::current_exe().expect("cache hygiene test executable"))
            .args([
                "--exact",
                "cache_hygiene::tests::default_prune_cache_handles_a_missing_home",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env_remove("HOME")
            .status()
            .expect("spawn HOME-free cache hygiene test");
    assert!(status.success(), "HOME-free child exited with {status}");
}

#[test]
fn process_liveness_and_prune_root_helpers() {
    assert!(process_is_alive(std::process::id() as i32));
    assert_eq!(
        live_coverage_pid(&format!("llvm-cov-{}", std::process::id())),
        Some(std::process::id() as i32)
    );
    assert!(live_coverage_pid("llvm-cov-not-a-pid").is_none());
    assert!(ensure_prune_root(Some(PathBuf::new())).is_err());
    assert_eq!(
        ensure_prune_root(Some(PathBuf::from("/tmp/cache"))).expect("explicit"),
        PathBuf::from("/tmp/cache")
    );
    assert!(
        ensure_prune_root(None)
            .expect("default")
            .ends_with(".cache")
    );
    assert!(default_prune_root().expect("home").ends_with(".cache"));
    assert_eq!(
        format_prune_summary(&PruneSummary {
            tagged_dirs: 2,
            adapter_logs: 3,
        }),
        "pruned 2 tagged cache dirs, 3 adapter logs"
    );
    let root = tempfile::tempdir().expect("prepare fixture");
    require_coverage_disk(root.path()).expect("fixture volume has coverage headroom");
    let prepared = root.path().join("llvm-cov-prep");
    prepare_coverage_target(&prepared).expect("prepare");
    assert!(has_cachedir_tag(&prepared));
    let missing = tempfile::tempdir().expect("missing prune root");
    let absent = missing.path().join("no-such");
    assert_eq!(
        prune_tagged_dirs(&absent, SystemTime::now()).expect("missing walk"),
        0
    );
}

#[test]
fn prune_keeps_a_parent_that_contains_live_coverage() {
    let root = tempfile::tempdir().expect("live parent");
    let parent = root.path().join("target");
    write_cachedir_tag(&parent).expect("tag parent");
    let live = parent.join(format!("llvm-cov-{}", std::process::id()));
    fs::create_dir(&live).expect("live child");
    age_path(
        &parent,
        TAGGED_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );
    assert_eq!(
        prune_tagged_dirs(root.path(), SystemTime::now()).expect("prune"),
        0
    );
    assert!(parent.is_dir());
}

#[test]
fn tagged_prune_root_stays_inside_fixtures_and_walks_home_cache() {
    let fixture = tempfile::tempdir().expect("fixture");
    assert_eq!(tagged_prune_root(fixture.path()), fixture.path());
    let home_cache = default_prune_root().expect("home cache");
    assert_eq!(tagged_prune_root(&home_cache.join("claudex")), home_cache);
}

#[test]
fn prune_tagged_cache_drops_stale_trees_and_keeps_fresh_mtime() {
    let root = tempfile::tempdir().expect("spawn prune fixture");
    let stale = root.path().join("old-target");
    let fresh = root.path().join("fresh-target");
    write_cachedir_tag(&stale).expect("tag stale");
    write_cachedir_tag(&fresh).expect("tag fresh");
    age_path(
        &stale,
        TAGGED_TARGET_RETENTION + std::time::Duration::from_secs(1),
    );
    prune_tagged_cache(root.path());
    assert!(!stale.exists());
    assert!(fresh.is_dir());
}

fn age_path(path: &std::path::Path, age: std::time::Duration) {
    fs::File::open(path)
        .expect("open")
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now() - age))
        .expect("age");
}
