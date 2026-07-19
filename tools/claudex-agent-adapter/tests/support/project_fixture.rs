use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ProjectFixture {
    path: PathBuf,
}

impl ProjectFixture {
    pub(crate) fn new(label: &str) -> Self {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("target/t").join(format!(
            "{}-{id}-{label}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create project-local test fixture");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProjectFixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "failed to clean project-local fixture {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}
