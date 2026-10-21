//! Streams a chat reply to stdout.
//!
//! ```text
//! ANTHROPIC_API_KEY=... cargo run -p synapse-ai --example chat -- anthropic "Say hi"
//! OPENAI_API_KEY=...    cargo run -p synapse-ai --example chat -- openai "Say hi"
//! GEMINI_API_KEY=...    cargo run -p synapse-ai --example chat -- gemini "Say hi"
//! cargo run -p synapse-ai --example chat -- ollama "Say hi" [model]   # no key needed
//! ```
//! The key is read from `<PROVIDER>_API_KEY` and is never printed.

use std::io::Write;

use synapse_ai::{
    AiClient, CancellationToken, ChatRequest, EnvKeyStore, ProviderConfig, ProviderKind,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let provider = args.next().unwrap_or_else(|| "anthropic".into());
    let prompt = args
        .next()
        .unwrap_or_else(|| "Say hello in one sentence.".into());
    let mut cfg = match provider.as_str() {
        "anthropic" => ProviderConfig::new(ProviderKind::Anthropic),
        "openai" => ProviderConfig::new(ProviderKind::OpenAi),
        "gemini" => ProviderConfig::new(ProviderKind::Gemini),
        "ollama" => ProviderConfig::new(ProviderKind::OpenAiCompatible),
        other => return Err(format!("unknown provider `{other}`").into()),
    };
    if let Some(model) = args.next() {
        cfg.model = model;
    }

    let ai = AiClient::new(&cfg, &EnvKeyStore)?;
    let cancel = CancellationToken::new();
    let mut sink = |t: &str| {
        print!("{t}");
        let _ = std::io::stdout().flush();
    };
    let r = ai
        .chat(
            &ChatRequest::new("You are a concise assistant.", prompt),
            Some(&mut sink),
            &cancel,
        )
        .await?;
    eprintln!(
        "\n[{} in / {} out tokens]",
        r.usage.input_tokens, r.usage.output_tokens
    );
    Ok(())
}
