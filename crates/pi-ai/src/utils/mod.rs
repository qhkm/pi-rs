/// Utility helpers for the `pi-ai` crate.
///
/// These are small, self-contained functions that support the provider and
/// streaming layers without depending on any particular backend.
pub mod partial_json;
pub mod proxy;

pub use partial_json::parse_partial_json;
pub use proxy::build_http_client;
