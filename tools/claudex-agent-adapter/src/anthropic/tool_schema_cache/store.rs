use std::{collections::VecDeque, path::PathBuf};

use super::persist::{
    StoreLock, bound_entries, create_private_directory, load_entries, load_entries_at, merge_entry,
    parent_directory, retain_fresh, write_entries,
};
use super::{CACHE_FILE_NAME, StoredSchema};


pub(super) struct ToolSchemaStore {
    path: PathBuf,
}

impl ToolSchemaStore {
    pub(super) fn for_current_user() -> Option<Self> {
        std::env::var_os("HOME").map(|home| Self {
            path: PathBuf::from(home)
                .join(".cache/claudex")
                .join(CACHE_FILE_NAME),
        })
    }

    #[cfg(test)]
    pub(super) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn load(&self) -> VecDeque<StoredSchema> {
        load_entries(&self.path)
    }

    pub(super) fn save(&self, current: StoredSchema, now: u64) -> StoredSchema {
        let fallback = current.clone();
        let result = create_private_directory(parent_directory(&self.path)).and_then(|()| {
            let _lock = StoreLock::acquire(&self.path)?;
            let mut merged = load_entries_at(&self.path, now);
            let persisted = merge_entry(&mut merged, current);
            retain_fresh(&mut merged, now);
            bound_entries(&mut merged);
            write_entries(&self.path, merged)?;
            Ok(persisted)
        });
        match result {
            Ok(persisted) => persisted,
            Err(error) => {
                tracing::warn!(%error, path = %self.path.display(), "could not persist Claude Code tool schemas");
                fallback
            }
        }
    }
}

