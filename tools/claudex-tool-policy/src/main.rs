use std::process::ExitCode;

fn main() -> ExitCode {
    match claudex_tool_policy::run() {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(err) => {
            eprintln!("claudex-tool-policy: {err}");
            ExitCode::from(1)
        }
    }
}
