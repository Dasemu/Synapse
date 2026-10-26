//! Synapse core: domain model, SQLite-backed graph store, link parsing, temporal (journal)
//! links, weighted synapses and a content-addressed blob store.
//!
//! Every mutation is appended to an op log (`Graph::ops_since`), which is the single change
//! feed used by indexers (search), backup (history) and sync.

pub mod blob;
pub mod embed;
pub mod error;
pub mod hlc;
pub mod model;
pub mod module_data;
pub mod parse;
pub mod position;
mod schema;
pub mod store;
pub mod structure;
pub mod synapse;
pub mod tags;
pub mod types;

pub use error::{Error, Result};
pub use model::*;
pub use module_data::{MAX_MODULE_KEY, MAX_MODULE_VALUE, is_valid_module_id};
pub use store::Graph;
pub use structure::{
    HubConversion, HubInfo, HubPreview, Structure, is_legacy_page, namespace_parent,
};
pub use tags::{MAX_TAG_CHARS, TagInfo, normalize_tag};
pub use types::{MAX_ICON_CHARS, SUGGESTED_TYPES, normalize_icon};
