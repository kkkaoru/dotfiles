use std::path::PathBuf;

use anyhow::{Result, bail};

use super::{DEFAULT_MAX_TURNS, DEFAULT_MODEL, LaunchSpec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    pub spec: LaunchSpec,
}

impl Options {
    pub fn parse<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut model = None;
        let mut effort = None;
        let mut program = None;
        let mut max_turns = DEFAULT_MAX_TURNS;
        let mut args = args.into_iter().peekable();
        while let Some(arg) = args.next() {
            match arg.as_ref() {
                "--model" => model = Some(require_value("--model", args.next())?),
                "--effort" => effort = Some(require_value("--effort", args.next())?),
                "--cmd" => program = Some(PathBuf::from(require_value("--cmd", args.next())?)),
                "--max-turns" => {
                    max_turns = parse_max_turns(&require_value("--max-turns", args.next())?)?;
                }
                unknown => bail!("unsupported command-code-acp argument: {unknown}"),
            }
        }
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        if model.trim().is_empty() {
            bail!("--model must not be empty");
        }
        let program = program
            .or_else(|| std::env::var_os("COMMAND_CODE_CMD").map(PathBuf::from))
            .unwrap_or_else(default_cmd_program);
        Ok(Self {
            spec: LaunchSpec {
                program,
                model,
                effort,
                max_turns,
                yolo: true,
                trust: true,
                skip_onboarding: true,
            },
        })
    }
}

fn default_cmd_program() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let wrapper = PathBuf::from(home).join(".local/bin/cmd");
        if wrapper.is_file() {
            return wrapper;
        }
    }
    PathBuf::from("cmd")
}

fn require_value(flag: &str, value: Option<impl AsRef<str>>) -> Result<String> {
    let value = value
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_default();
    if value.is_empty() || value.starts_with("--") {
        bail!("{flag} requires a value");
    }
    Ok(value)
}

fn parse_max_turns(value: &str) -> Result<u32> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("--max-turns must be a positive integer"))?;
    if parsed == 0 {
        bail!("--max-turns must be a positive integer");
    }
    Ok(parsed)
}
