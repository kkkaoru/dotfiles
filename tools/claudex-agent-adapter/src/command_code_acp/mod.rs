//! Official Command Code headless (`cmd -p`) exposed as ACP stdio.
//!
//! Command Code has no native ACP. Claudex reuses `configured-acp` by launching
//! this shim, which speaks ACP and drives `cmd -p --output-format json`.

mod agent;
mod events;
mod launch;
mod options;
mod process;
mod prompt;

pub use agent::serve;
pub use events::{ParsedLine, ProgressEvent, TurnResult, parse_stdout_line, progress_to_updates};
pub use launch::{DEFAULT_MAX_TURNS, DEFAULT_MODEL, LaunchSpec};
pub use options::Options;
pub use process::run_turn;
pub use prompt::prompt_text;

use anyhow::Result;

/// Parse process args and serve ACP on stdin/stdout.
pub async fn run() -> Result<()> {
    let options = Options::parse(std::env::args().skip(1))?;
    serve(options).await
}

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod tests;
