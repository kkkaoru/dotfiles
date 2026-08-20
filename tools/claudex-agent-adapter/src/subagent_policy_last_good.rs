use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::valid_model_id;

#[cfg_attr(test, allow(dead_code))]
const LAST_GOOD_FILE_NAME: &str = "disabled-subagent-models.last-good.json";
const LAST_GOOD_VERSION: i64 = 1;

#[cfg(test)]
thread_local! {
    static OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn persist(source: &Path, models: &BTreeSet<String>) {
    let Some(path) = last_good_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = json!({
        "version": LAST_GOOD_VERSION,
        "source": source.to_string_lossy(),
        "disabledModels": models.iter().cloned().collect::<Vec<_>>(),
    });
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        return;
    };
    atomic_write(&path, &bytes);
}

pub(super) fn restore(source: &Path) -> Option<BTreeSet<String>> {
    let path = last_good_file()?;
    let text = fs::read_to_string(path).ok()?;
    parse_stored(&text, source)
}

fn parse_stored(text: &str, source: &Path) -> Option<BTreeSet<String>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.get("version")?.as_i64()? != LAST_GOOD_VERSION {
        return None;
    }
    if object.get("source")?.as_str()? != source.to_string_lossy() {
        return None;
    }
    let models = object.get("disabledModels")?.as_array()?;
    let mut set = BTreeSet::new();
    for model in models {
        let text = model.as_str()?;
        if !valid_model_id(text) {
            return None;
        }
        set.insert(text.to_owned());
    }
    Some(set)
}

fn last_good_file() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(test_last_good_file())
    }
    #[cfg(not(test))]
    {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".cache/claudex")
                .join(LAST_GOOD_FILE_NAME)
        })
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let wrote = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        });
    if wrote.is_ok() {
        let _ = fs::rename(&temporary, path);
    }
    let _ = fs::remove_file(&temporary);
}

#[cfg(test)]
fn test_last_good_file() -> PathBuf {
    OVERRIDE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(fresh_test_path());
        }
        slot.clone().expect("test last-good path")
    })
}

#[cfg(test)]
fn fresh_test_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "claudex-denylist-last-good-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

#[cfg(test)]
pub(super) fn reset_test_file() {
    OVERRIDE.with(|slot| {
        if let Some(path) = slot.borrow().as_ref() {
            let _ = fs::remove_file(path);
        }
        *slot.borrow_mut() = Some(fresh_test_path());
    });
}

#[cfg(test)]
pub(super) fn keep_test_file() {
    OVERRIDE.with(|slot| {
        if slot.borrow().is_none() {
            *slot.borrow_mut() = Some(fresh_test_path());
        }
    });
}
