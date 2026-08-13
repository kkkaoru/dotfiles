fn main() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if std::env::args().nth(1).as_deref() == Some("report-only") {
        claudex_agent_adapter::coverage_gate::report(root)
    } else {
        claudex_agent_adapter::coverage_gate::run(root)
    }
}
