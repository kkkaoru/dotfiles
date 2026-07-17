#[path = "src/build_support.rs"]
mod build_support;

fn main() {
    build_support::emit_build_metadata(std::path::Path::new("."));
}
