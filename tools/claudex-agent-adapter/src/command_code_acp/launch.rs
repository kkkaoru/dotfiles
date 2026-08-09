use std::path::{Path, PathBuf};

pub const DEFAULT_MODEL: &str = "meta/muse-spark-1.2-contributor";
pub const DEFAULT_MAX_TURNS: u32 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub model: String,
    pub effort: Option<String>,
    pub max_turns: u32,
    pub yolo: bool,
    pub trust: bool,
    pub skip_onboarding: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnLaunch<'a> {
    pub spec: &'a LaunchSpec,
    pub prompt: &'a str,
    pub resume: Option<&'a str>,
}

impl LaunchSpec {
    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn argv(&self, prompt: &str, resume: Option<&str>) -> Vec<String> {
        TurnLaunch {
            spec: self,
            prompt,
            resume,
        }
        .argv()
    }
}

impl TurnLaunch<'_> {
    pub fn argv(&self) -> Vec<String> {
        let mut args = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "json".to_owned(),
            "--model".to_owned(),
            self.spec.model.clone(),
            "--max-turns".to_owned(),
            self.spec.max_turns.to_string(),
        ];
        if self.spec.skip_onboarding {
            args.push("--skip-onboarding".to_owned());
        }
        if self.spec.yolo {
            args.push("--yolo".to_owned());
        }
        if self.spec.trust {
            args.push("--trust".to_owned());
        }
        // Muse Spark SubAgents must not inherit Command Code skill dumps.
        // `--no-session` skips persisting this run; ACP still always launches
        // without `--resume` so dirty-repo greetings are not replayed.
        // `--no-session` is incompatible with `--resume`.
        args.push("--no-skills".to_owned());
        // Muse Spark 1.2 Contributor rejects `--effort` ("no adjustable reasoning
        // effort"). Keep effort on the ACP shim for TUI status only.
        match self.resume {
            Some(session_id) if !session_id.is_empty() => {
                args.push("--resume".to_owned());
                args.push(session_id.to_owned());
            }
            _ => args.push("--no-session".to_owned()),
        }
        args.push(self.prompt.to_owned());
        args
    }
}
