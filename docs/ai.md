# AI (`synapse-ai`)

Semi-integrated AI: the user picks a provider and pastes their own API key. Nothing is sent
anywhere unless a feature is invoked.

## Providers

| `ProviderKind` | Chat (streaming) | Embeddings | Default endpoint |
|---|---|---|---|
| `Anthropic` | Messages API, default model `claude-opus-5-5` | **none** (Anthropic has no embeddings API; use OpenAI, Gemini, Ollama or Voyage) | `api.anthropic.com` |
| `OpenAi` | Chat Completions | `text-embedding-3-small` | `api.openai.com/v1` |
| `Gemini` | `streamGenerateContent` | `gemini-embedding-001` | `generativelanguage.googleapis.com/v1beta` |
| `OpenAiCompatible` | Ollama, LM Studio, OpenRouter, Mistral… (set `base_url`) | any `/embeddings` endpoint, e.g. Ollama `nomic-embed-text`, Voyage | `localhost:11434/v1` |

Mixing is done by configuring two `ProviderConfig`s (e.g. Anthropic for chat, Ollama for
embeddings) and building two `AiClient`s.

## Config and keys

`ProviderConfig` (serde) holds `kind, base_url, model, embed_model, embed_dims, key_name,
max_tokens, temperature, timeout_secs, max_retries, min_request_interval_ms`. It never contains a
key. Keys come from a `KeyStore` (`get(name) -> Option<Secret>`): the app implements it over the
OS keystore; `MemoryKeyStore` and `EnvKeyStore` (`<NAME>_API_KEY`) ship for tests/CLI. `Secret`
redacts itself in `Debug`; keys travel only in HTTP headers (also for Gemini).

## Calling

```rust
let ai = AiClient::new(&cfg, &keys)?;                       // async API, runs on the caller's tokio runtime
let r = ai.chat(&ChatRequest::new(system, user), Some(&mut |t| print!("{t}")), &cancel).await?;
r.usage; ai.total_usage();                                   // token reporting
let embedder = ai.blocking_embedder()?;                      // impl synapse_core::embed::Embedder
```

- HTTP is `reqwest` + rustls (ring, bundled Mozilla roots): no OpenSSL, no JNI, builds for
  Android/Windows.
- Retries 408/409/429/5xx and network errors with exponential backoff + jitter, honouring
  `Retry-After`; streaming is not retried once bytes have arrived. `min_request_interval_ms`
  throttles client-side.
- Cancel with `CancellationToken` (returns `Error::Cancelled`).
- `BlockingEmbedder` owns a one-thread runtime and blocks the caller (from async code use
  `spawn_blocking`). `dims()` is known for common models or discovered with one probe request.

## Graph features (`synapse_ai::features`)

Each is a sync step that reads the `Graph` and returns a `Job`, then `job.run(&ai, sink, &cancel).await`
returns `Outcome { value, usage }`. (This keeps `&Graph` out of futures, since `Graph` is not `Sync`.)

| Function | Result |
|---|---|
| `ask_graph(graph, question, &retrieve, &opts)` | `Answer { text, citations, context }`; `retrieve: Fn(&str) -> Vec<BlockId>` is supplied by the caller (plug `synapse-search` in); notes are given to the model as `<note id="((uuid))">` and only ids actually in the context are returned as citations |
| `summarize_journal(graph, from, to)` | Markdown recap; no model call if the days are empty |
| `suggest_links_and_tags(graph, page)` | `Suggestions { links (exists?, block), tags }`, validated against the graph |
| `extract_tasks(graph, block)` + `insert_tasks` | `TaskSuggestion` → `TODO [#A] … / DEADLINE:` child blocks |
| `consolidate_week(graph, week_start)` + `apply_proposal` | `NoteProposal { title, blocks, tags, sources }` → new page |

Nothing is written to the graph until `insert_tasks` / `apply_proposal` is called. Prompts live in
`prompts.rs`: English, "answer in the user's language", note text framed as data.
Run `cargo run -p synapse-ai --example chat -- anthropic "hi"` with `ANTHROPIC_API_KEY` set.
