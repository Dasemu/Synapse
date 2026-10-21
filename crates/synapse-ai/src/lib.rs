//! Provider-agnostic AI for Synapse: streaming chat, embeddings and graph-aware features
//! (RAG, summaries, link/tag suggestions, task extraction, weekly consolidation).
//!
//! API keys come from a [`KeyStore`] and never appear in [`ProviderConfig`], the graph or logs.

mod client;
mod config;
mod error;
pub mod features;
mod http;
mod keys;
pub mod prompts;
mod provider;
pub mod providers;
mod types;

pub use client::{AiClient, BlockingEmbedder};
pub use config::{ProviderConfig, ProviderKind};
pub use error::{Error, Result};
pub use keys::{EnvKeyStore, KeyStore, MemoryKeyStore, Secret};
pub use provider::{ChatProvider, EmbeddingProvider};
pub use tokio_util::sync::CancellationToken;
pub use types::*;
