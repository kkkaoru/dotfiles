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
    if arguments.get(1).is_some_and(|flag| {
        flag.to_string_lossy() == claudex_agent_adapter::anthropic::pi_tools_mcp::PI_TOOLS_MCP_FLAG
    }) {
        return claudex_agent_adapter::anthropic::pi_tools_mcp::serve_stdio().await;
    }
    let code = claudex_agent_adapter::runtime::run(arguments).await?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
