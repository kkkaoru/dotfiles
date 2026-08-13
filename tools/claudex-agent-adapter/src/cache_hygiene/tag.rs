use std::{fs, path::Path};

use anyhow::{Context, Result};

use super::{CACHEDIR_TAG_CONTENTS, CACHEDIR_TAG_NAME};

pub(crate) fn write_cachedir_tag(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    fs::write(dir.join(CACHEDIR_TAG_NAME), CACHEDIR_TAG_CONTENTS)
        .with_context(|| format!("write {} in {}", CACHEDIR_TAG_NAME, dir.display()))
}

pub(crate) fn has_cachedir_tag(dir: &Path) -> bool {
    dir.join(CACHEDIR_TAG_NAME).is_file()
}
