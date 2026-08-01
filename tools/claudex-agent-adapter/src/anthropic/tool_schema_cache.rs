use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{MessagesRequest, RequestIdentity};

const CACHE_FILE_NAME: &str = "tool-schemas-v1.json";
const CACHE_VERSION: u8 = 1;
const MAX_ENTRIES: usize = 1_024;
const MAX_AGE_SECONDS: u64 = 2 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SchemaIdentity {
    session_id: String,
    agent_id: Option<String>,
    parent_agent_id: Option<String>,
}

impl SchemaIdentity {
    fn from_request(identity: &RequestIdentity) -> Option<Self> {
        Some(Self {
            session_id: identity.session_id()?.to_owned(),
            agent_id: identity.agent_id().map(str::to_owned),
            parent_agent_id: identity.parent_agent_id().map(str::to_owned),
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredSchema {
    identity: SchemaIdentity,
    tools: Vec<Value>,
    /// Schema generation. Reads must not advance this value because a stale
    /// daemon could otherwise replace a newer schema merely by accessing it.
    updated_unix_seconds: u64,
    #[serde(default)]
    accessed_unix_seconds: u64,
}

#[derive(Default, Deserialize, Serialize)]
struct StoredSchemas {
    version: u8,
    entries: Vec<StoredSchema>,
}

pub(super) struct ToolSchemaCache {
    entries: Mutex<VecDeque<StoredSchema>>,
    store: Option<ToolSchemaStore>,
}

impl Default for ToolSchemaCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            store: None,
        }
    }
}

impl ToolSchemaCache {
    pub(super) fn persistent() -> Self {
        let Some(store) = ToolSchemaStore::for_current_user() else {
            return Self::default();
        };
        Self {
            entries: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    #[cfg(test)]
    fn with_store(path: PathBuf) -> Self {
        let store = ToolSchemaStore::at(path);
        Self {
            entries: Mutex::new(store.load()),
            store: Some(store),
        }
    }

    pub(super) fn restore_or_remember(
        &self,
        identity: &RequestIdentity,
        request: &mut MessagesRequest,
        tools_were_provided: bool,
    ) {
        self.restore_or_remember_at(identity, request, tools_were_provided, unix_seconds());
    }

    fn restore_or_remember_at(
        &self,
        identity: &RequestIdentity,
        request: &mut MessagesRequest,
        tools_were_provided: bool,
        now: u64,
    ) {
        let Some(identity) = SchemaIdentity::from_request(identity) else {
            return;
        };
        let mut entries = self.entries.lock().expect("tool schema cache poisoned");
        retain_fresh(&mut entries, now);
        if !tools_were_provided {
            self.restore_cached_schema(&mut entries, &identity, request, now);
            return;
        }
        // An explicit empty array is an authoritative zero-capability request.
        // It neither restores nor mutates a previously remembered schema.
        if request.tools.is_empty() {
            return;
        }

        entries.retain(|entry| entry.identity != identity);
        let stored = self.persist(
            StoredSchema {
                identity,
                tools: request.tools.clone(),
                updated_unix_seconds: now,
                accessed_unix_seconds: now,
            },
            now,
        );
        entries.push_back(stored);
        bound_entries(&mut entries);
    }

    fn restore_cached_schema(
        &self,
        entries: &mut VecDeque<StoredSchema>,
        identity: &SchemaIdentity,
        request: &mut MessagesRequest,
        now: u64,
    ) {
        if !request.tools.is_empty() {
            return;
        }
        let Some(index) = entries
            .iter()
            .rposition(|entry| entry.identity == *identity)
        else {
            return;
        };
        let mut stored = entries.remove(index).expect("matched schema entry");
        stored.accessed_unix_seconds = now;
        let stored = self.persist(stored, now);
        request.tools.clone_from(&stored.tools);
        entries.push_back(stored);
        bound_entries(entries);
    }

    fn persist(&self, stored: StoredSchema, now: u64) -> StoredSchema {
        match &self.store {
            Some(store) => store.save(stored, now),
            None => stored,
        }
    }
}

struct ToolSchemaStore {
    path: PathBuf,
}

impl ToolSchemaStore {
    fn for_current_user() -> Option<Self> {
        std::env::var_os("HOME").map(|home| Self {
            path: PathBuf::from(home)
                .join(".cache/claudex")
                .join(CACHE_FILE_NAME),
        })
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> VecDeque<StoredSchema> {
        load_entries(&self.path)
    }

    fn save(&self, current: StoredSchema, now: u64) -> StoredSchema {
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

fn load_entries(path: &Path) -> VecDeque<StoredSchema> {
    load_entries_at(path, unix_seconds())
}

fn load_entries_at(path: &Path, now: u64) -> VecDeque<StoredSchema> {
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

fn merge_entry(entries: &mut VecDeque<StoredSchema>, current: StoredSchema) -> StoredSchema {
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

fn schema_sort_key(tools: &[Value]) -> Vec<u8> {
    serde_json::to_vec(tools).unwrap_or_default()
}

fn write_entries(path: &Path, entries: VecDeque<StoredSchema>) -> std::io::Result<()> {
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

fn retain_fresh(entries: &mut VecDeque<StoredSchema>, now: u64) {
    entries.retain(|entry| {
        let last_accessed = entry.accessed_unix_seconds.max(entry.updated_unix_seconds);
        !entry.identity.session_id.is_empty()
            && !entry.tools.is_empty()
            && now.saturating_sub(last_accessed) <= MAX_AGE_SECONDS
    });
}

fn bound_entries(entries: &mut VecDeque<StoredSchema>) {
    while entries.len() > MAX_ENTRIES {
        entries.pop_front();
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
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

struct StoreLock {
    file: File,
}

impl StoreLock {
    fn acquire(cache_path: &Path) -> std::io::Result<Self> {
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

#[cfg(test)]
mod tests;
