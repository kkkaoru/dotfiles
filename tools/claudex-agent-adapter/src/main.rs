#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let code = claudex_app_server_adapter::runtime::run(std::env::args_os()).await?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
