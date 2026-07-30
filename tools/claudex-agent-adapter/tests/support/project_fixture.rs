use std::{
    path::{Path, PathBuf},
    sync::{
        Once,
        atomic::{AtomicU64, Ordering},
    },
};

use tempfile::TempDir;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
static LEGACY_TARGET_CLEANUP: Once = Once::new();

pub(crate) struct ProjectFixture {
    _root: TempDir,
    path: PathBuf,
}

impl ProjectFixture {
    pub(crate) fn new(label: &str) -> Self {
        cleanup_legacy_target_fixtures();
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = tempfile::tempdir_in("/tmp").expect("create temporary project fixture root");
        let path = root.path().join(format!("cx-{id}-{label}"));
        std::fs::create_dir_all(&path).expect("create project-local test fixture");
        Self { _root: root, path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn cleanup_legacy_target_fixtures() {
    LEGACY_TARGET_CLEANUP.call_once(|| {
        let legacy_root = Path::new("target/t");
        if let Err(error) = std::fs::remove_dir_all(legacy_root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "failed to clean legacy project fixtures {}: {error}",
                legacy_root.display()
            );
        }
    });
}
