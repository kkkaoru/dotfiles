use std::path::Path;

use super::Event;
#[cfg(test)]
use super::TEST_FORCE_DELEGATE;
use crate::launcher::installed_adapter;
use crate::launcher::macos_notify::NotifyKind;

pub(super) fn delegate_post(cache: &Path, event: &Event) -> bool {
    #[cfg(test)]
    {
        // Keep unit suites in-process by default. Opt into the production
        // spawn path explicitly so delegation stays measurable under llvm-cov.
        if !TEST_FORCE_DELEGATE.with(std::cell::Cell::get) {
            let _ = (cache, event);
            return false;
        }
    }
    delegate_complete_notify(cache, event)
}

/// Spawn the installed adapter to post a completion banner out-of-process.
pub(super) fn delegate_complete_notify(cache: &Path, event: &Event) -> bool {
    use std::process::Command;

    if event.kind() != NotifyKind::Complete {
        return false;
    }
    let Some(exe) = installed_adapter::notify_delegate_executable() else {
        return false;
    };
    let Some(cache) = cache.to_str() else {
        return false;
    };
    interpret_delegate_status(
        Command::new(&exe)
            .env(installed_adapter::NOTIFY_IN_PROCESS_ENV, "1")
            .args([
                "__internal-notify",
                "complete",
                cache,
                event.listen(),
                event.build_id(),
            ])
            .status(),
    )
}

pub(super) fn interpret_delegate_status(result: std::io::Result<std::process::ExitStatus>) -> bool {
    match result {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("claudex: delegated macOS notify exited {status}");
            false
        }
        Err(error) => {
            eprintln!("claudex: delegated macOS notify failed ({error})");
            false
        }
    }
}
