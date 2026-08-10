#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if arguments
        .get(1)
        .is_some_and(|flag| flag.to_string_lossy() == "__internal-notify")
    {
        claudex_agent_adapter::launcher::run_internal_notify(arguments)?;
        return Ok(());
    }
    let code = claudex_agent_adapter::runtime::run(arguments).await?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
