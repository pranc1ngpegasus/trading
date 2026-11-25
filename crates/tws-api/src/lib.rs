// Generated code is not lint-clean: buffa emits `use_self`, `doc_markdown`,
// `cast_lossless` and similar throughout. Seal the clippy namespace only, so the
// compiler's own lints keep guarding generated code. The workspace's
// `unsafe_code = "deny"` is a rustc lint and survives this attribute.
#[allow(clippy::all, clippy::nursery, clippy::pedantic, clippy::restriction)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/_include.rs"));
}

pub use buffa;
pub use generated::protobuf as proto;

/// TWS API version these types were generated from, read at build time.
pub const API_VERSION: &str = env!("TWS_API_VERSION");
