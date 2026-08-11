use std::io::{BufRead, BufReader, Write};

use anyhow::Result;

pub(super) fn relay_filtered_io(
    input: &mut dyn std::io::Read,
    model: &str,
    output: &mut dyn Write,
) -> Result<()> {
    let advisor_warning = format!("Advisor disabled — base model '{model}' has no advisor rank");
    let connector_warning = "claude.ai connectors are disabled because";
    let mut reader = BufReader::new(input);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? > 0 {
        let text = String::from_utf8_lossy(&line);
        if !text.contains(&advisor_warning) && !text.contains(connector_warning) {
            output.write_all(&line)?;
            output.flush()?;
        }
        line.clear();
    }
    Ok(())
}

pub(super) fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map_or(1, |signal| 128 + signal)
        }
        #[cfg(not(unix))]
        {
            1
        }
    })
}
