use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

use super::{
    CACHE_VERSION, MAX_AGE_SECONDS, MAX_ENTRIES, StoredSchema, StoredSchemas, schema_sort_key,
};

pub(super) fn load_entries(path: &Path) -> VecDeque<StoredSchema> {
    load_entries_at(path, super::unix_seconds())
}

pub(super) fn load_entries_at(path: &Path, now: u64) -> VecDeque<StoredSchema> {
    let Ok(bytes) = fs::read(path) else {
        return VecDeque::new();
    };
    let Ok(stored) = serde_json::from_slice::<StoredSchemas>(&bytes) else {
        tracing::warn!(path = %path.display(), "could not decode persisted Claude Code tool schemas");
        return VecDeque::new();
    };
    if stored.version != CACHE_VERSION {
        tracing::warn!(path = %path.display(), "ignored incompatible Claude Code tool schema cache");
        return VecDeque::new();
    }
    let mut entries = VecDeque::from(stored.entries);
    retain_fresh(&mut entries, now);
    bound_entries(&mut entries);
    entries
}

pub(super) fn merge_entry(
    entries: &mut VecDeque<StoredSchema>,
    current: StoredSchema,
) -> StoredSchema {
    let merged = match entries
        .iter()
        .position(|stored| stored.identity == current.identity)
        .and_then(|index| entries.remove(index))
    {
        Some(stored) => merge_generation(stored, current),
        None => current,
    };
    entries.push_back(merged.clone());
    merged
}

fn merge_generation(stored: StoredSchema, current: StoredSchema) -> StoredSchema {
    let accessed_unix_seconds = stored
        .accessed_unix_seconds
        .max(stored.updated_unix_seconds)
        .max(current.accessed_unix_seconds)
        .max(current.updated_unix_seconds);
    let current_is_newer = current.updated_unix_seconds > stored.updated_unix_seconds
        || (current.updated_unix_seconds == stored.updated_unix_seconds
            && schema_sort_key(&current.tools) >= schema_sort_key(&stored.tools));
    let mut selected = if current_is_newer { current } else { stored };
    selected.accessed_unix_seconds = accessed_unix_seconds;
    selected
}

pub(super) fn write_entries(path: &Path, entries: VecDeque<StoredSchema>) -> std::io::Result<()> {
    let document = StoredSchemas {
        version: CACHE_VERSION,
        entries: entries.into(),
    };
    let bytes = serde_json::to_vec(&document).map_err(std::io::Error::other)?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = write_private(&temporary, &bytes).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn retain_fresh(entries: &mut VecDeque<StoredSchema>, now: u64) {
    entries.retain(|entry| {
        let last_accessed = entry.accessed_unix_seconds.max(entry.updated_unix_seconds);
        !entry.identity.session_id.is_empty()
            && !entry.tools.is_empty()
            && now.saturating_sub(last_accessed) <= MAX_AGE_SECONDS
    });
}

pub(super) fn bound_entries(entries: &mut VecDeque<StoredSchema>) {
    while entries.len() > MAX_ENTRIES {
        entries.pop_front();
    }
}

pub(super) fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub(super) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

pub(super) struct StoreLock {
    file: File,
}

impl StoreLock {
    pub(super) fn acquire(cache_path: &Path) -> std::io::Result<Self> {
        let lock_path = cache_path.with_extension("lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)?;
        lock_exclusive(&file)?;
        Ok(Self { file })
    }
}

#[cfg(unix)]
fn lock_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: `file` owns a valid descriptor for the duration of this call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn lock_exclusive(_file: &File) -> std::io::Result<()> {
    Ok(())
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: the descriptor remains valid until this guard is dropped.
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}
