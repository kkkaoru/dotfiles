fn main() -> anyhow::Result<()> {
    claudex_agent_adapter::coverage_gate::run(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
}
