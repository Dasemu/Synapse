//! FFI surface of Synapse for the Flutter app (flutter_rust_bridge v2). The app never touches
//! storage directly: everything goes through [`api`]. Bindings are generated from `api` with
//! `cd app && flutter_rust_bridge_codegen generate`.

pub mod api;
#[rustfmt::skip]
mod frb_generated;
