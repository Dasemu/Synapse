use std::time::Duration;

use serde_json::json;
use synapse_ai::*;
use synapse_core::embed::Embedder;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn keys() -> MemoryKeyStore {
    let k = MemoryKeyStore::new();
    for n in ["anthropic", "openai", "gemini"] {
        k.set(n, "test-key");
    }
    k
}

fn cfg(kind: ProviderKind, server: &MockServer) -> ProviderConfig {
    ProviderConfig {
        base_url: Some(server.uri()),
        ..ProviderConfig::new(kind)
    }
}

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn req() -> ChatRequest {
    ChatRequest::new("be brief", "hello")
}

#[tokio::test]
async fn anthropic_streams_text_and_usage() {
    let server = MockServer::start().await;
    let body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(sse(body))
        .expect(1)
        .mount(&server)
        .await;

    let ai = AiClient::new(&cfg(ProviderKind::Anthropic, &server), &keys()).unwrap();
    let mut tokens = Vec::new();
    let mut sink = |t: &str| tokens.push(t.to_string());
    let r = ai
        .chat(&req(), Some(&mut sink), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.text, "Hello");
    assert_eq!(tokens, ["Hel", "lo"]);
    assert_eq!(
        r.usage,
        Usage {
            input_tokens: 11,
            output_tokens: 7
        }
    );
    assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(ai.total_usage().total(), 18);

    let sent: serde_json::Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(sent["model"], "claude-opus-5-5");
    assert_eq!(sent["system"], "be brief");
    assert!(sent.get("temperature").is_none());
}

#[tokio::test]
async fn anthropic_refusal_is_an_error() {
    let server = MockServer::start().await;
    let body = "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\"},\"usage\":{\"output_tokens\":0}}\n\n";
    Mock::given(method("POST"))
        .respond_with(sse(body))
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::Anthropic, &server), &keys()).unwrap();
    let e = ai
        .chat(&req(), None, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Refused(_)));
}

#[tokio::test]
async fn openai_streams_and_reports_usage() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n\
data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse(body))
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::OpenAi, &server), &keys()).unwrap();
    let mut r = req();
    r.json = true;
    let out = ai.chat(&r, None, &CancellationToken::new()).await.unwrap();
    assert_eq!(out.text, "Hi");
    assert_eq!(
        out.usage,
        Usage {
            input_tokens: 5,
            output_tokens: 2
        }
    );
    let sent: serde_json::Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(sent["response_format"]["type"], "json_object");
    assert!(sent.get("max_completion_tokens").is_some() && sent.get("max_tokens").is_none());
}

#[tokio::test]
async fn compatible_endpoint_needs_no_key_and_uses_max_tokens() {
    let server = MockServer::start().await;
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse(body))
        .mount(&server)
        .await;
    let ai = AiClient::new(
        &cfg(ProviderKind::OpenAiCompatible, &server),
        &MemoryKeyStore::new(),
    )
    .unwrap();
    assert_eq!(
        ai.chat(&req(), None, &CancellationToken::new())
            .await
            .unwrap()
            .text,
        "ok"
    );
    let rq = &server.received_requests().await.unwrap()[0];
    assert!(rq.headers.get("authorization").is_none());
    let sent: serde_json::Value = serde_json::from_slice(&rq.body).unwrap();
    assert!(sent.get("max_tokens").is_some());
}

#[tokio::test]
async fn gemini_streams_with_key_in_header() {
    let server = MockServer::start().await;
    let body = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Ho\"}]}}]}\n\n\
data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"la\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":3}}\n\n";
    Mock::given(method("POST"))
        .and(path("/models/gemini-2.5-flash:streamGenerateContent"))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(sse(body))
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::Gemini, &server), &keys()).unwrap();
    let out = ai
        .chat(&req(), None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out.text, "Hola");
    assert_eq!(
        out.usage,
        Usage {
            input_tokens: 4,
            output_tokens: 3
        }
    );
    let rq = &server.received_requests().await.unwrap()[0];
    assert!(!rq.url.as_str().contains("test-key"));
}

#[tokio::test]
async fn retries_429_honouring_retry_after_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_string(r#"{"error":{"message":"slow down"}}"#),
        )
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(sse(
            "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\ndata: [DONE]\n\n",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::OpenAi, &server), &keys()).unwrap();
    assert_eq!(
        ai.chat(&req(), None, &CancellationToken::new())
            .await
            .unwrap()
            .text,
        "done"
    );
}

#[tokio::test]
async fn gives_up_after_max_retries_and_does_not_retry_client_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "0"))
        .expect(3) // 1 attempt + 2 retries
        .mount(&server)
        .await;
    let c = ProviderConfig {
        max_retries: 2,
        ..cfg(ProviderKind::OpenAi, &server)
    };
    let ai = AiClient::new(&c, &keys()).unwrap();
    let e = ai
        .chat(&req(), None, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Api { status: 503, .. }));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"error":{"message":"bad key"}}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::OpenAi, &server), &keys()).unwrap();
    match ai
        .chat(&req(), None, &CancellationToken::new())
        .await
        .unwrap_err()
    {
        Error::Api {
            status: 401,
            message,
            ..
        } => assert_eq!(message, "bad key"),
        e => panic!("unexpected {e:?}"),
    }
}

#[tokio::test]
async fn cancellation_aborts_a_slow_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse("data: [DONE]\n\n").set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::OpenAi, &server), &keys()).unwrap();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        c2.cancel();
    });
    let t = std::time::Instant::now();
    assert!(matches!(
        ai.chat(&req(), None, &cancel).await,
        Err(Error::Cancelled)
    ));
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn openai_embeddings_reorder_by_index_and_blocking_embedder_works() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"index": 1, "embedding": [0.0, 1.0, 0.0]}, {"index": 0, "embedding": [1.0, 0.0, 0.0]}],
            "usage": {"prompt_tokens": 6}
        })))
        .mount(&server)
        .await;
    let c = ProviderConfig {
        embed_dims: Some(3),
        ..cfg(ProviderKind::OpenAi, &server)
    };
    let ai = AiClient::new(&c, &keys()).unwrap();
    let texts = vec!["a".to_string(), "b".to_string()];
    let r = ai.embed(&texts, &CancellationToken::new()).await.unwrap();
    assert_eq!(r.vectors[0], [1.0, 0.0, 0.0]);
    assert_eq!(r.usage.input_tokens, 6);

    let emb = ai.blocking_embedder().unwrap();
    assert_eq!(emb.model_id(), "openai/text-embedding-3-small");
    // The mock server lives on this runtime, so block off-thread.
    let v = tokio::task::spawn_blocking(move || {
        assert_eq!(emb.dims(), 3);
        emb.embed(&texts).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(v[1], [0.0, 1.0, 0.0]);
}

#[tokio::test]
async fn dims_are_probed_when_unknown() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data": [{"index": 0, "embedding": [0.1, 0.2]}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = ProviderConfig {
        embed_model: Some("custom-model".into()),
        ..cfg(ProviderKind::OpenAiCompatible, &server)
    };
    let ai = AiClient::new(&c, &MemoryKeyStore::new()).unwrap();
    let emb = ai.blocking_embedder().unwrap();
    let dims = tokio::task::spawn_blocking(move || (emb.dims(), emb.dims()))
        .await
        .unwrap();
    assert_eq!(dims, (2, 2));
}

#[tokio::test]
async fn gemini_batch_embeddings() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-embedding-001:batchEmbedContents"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"embeddings": [{"values": [1.0, 2.0]}, {"values": [3.0, 4.0]}]}),
            ),
        )
        .mount(&server)
        .await;
    let ai = AiClient::new(&cfg(ProviderKind::Gemini, &server), &keys()).unwrap();
    let r = ai
        .embed(&["x".into(), "y".into()], &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
}

#[test]
fn anthropic_has_no_embeddings() {
    let ai = AiClient::new(&ProviderConfig::new(ProviderKind::Anthropic), &keys()).unwrap();
    match ai.blocking_embedder() {
        Err(Error::Unsupported(m)) => assert!(m.contains("Voyage")),
        _ => panic!("expected Unsupported"),
    }
}

#[test]
fn keys_and_config_hygiene() {
    assert!(matches!(
        AiClient::new(&ProviderConfig::new(ProviderKind::OpenAi), &MemoryKeyStore::new()),
        Err(Error::MissingKey(n)) if n == "openai"
    ));
    assert_eq!(EnvKeyStore::var_name("open-router"), "OPEN_ROUTER_API_KEY");
    assert_eq!(format!("{:?}", Secret::new("hunter2")), "Secret(***)");

    let c = ProviderConfig {
        key_name: Some("work".into()),
        ..ProviderConfig::new(ProviderKind::Gemini)
    };
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains("\"kind\":\"gemini\""));
    assert_eq!(serde_json::from_str::<ProviderConfig>(&json).unwrap(), c);
    // Partial configs from the app fall back to defaults.
    let p: ProviderConfig = serde_json::from_str(r#"{"kind":"open_ai","model":"x"}"#).unwrap();
    assert_eq!(p.max_retries, 3);
}
