fn main() -> anyhow::Result<()> {
    claudex_app_server_adapter::coverage_gate::run(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
}
