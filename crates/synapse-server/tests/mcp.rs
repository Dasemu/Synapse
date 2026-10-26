//! MCP over Streamable HTTP: auth, tool calls, read-only vs write.

use serde_json::{Value, json};
use std::time::Duration;
use synapse_core::{Graph, NewPage, Placement};
use synapse_server::config::{DEFAULT_MCP_HOSTS, McpConfig};
use synapse_server::{Config, RunningServer};

const SYNC_TOKEN: &str = "sync-token-0123456789abcdef";
const MCP_TOKEN: &str = "mcp-token-0123456789abcdef";

async fn start(dir: &std::path::Path, allow_write: bool) -> RunningServer {
    let cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.join("data"),
        auth_tokens: vec![SYNC_TOKEN.into()],
        update_token: None,
        update_script: "/nonexistent".into(),
        max_clock_skew: Duration::from_secs(600),
        mcp: Some(McpConfig {
            tokens: vec![MCP_TOKEN.into()],
            allow_write,
            allowed_hosts: DEFAULT_MCP_HOSTS.iter().map(|h| h.to_string()).collect(),
        }),
    };
    cfg.validate().unwrap();
    synapse_server::start(&cfg).await.unwrap()
}

fn seed(dir: &std::path::Path) {
    let g = Graph::open(dir.join("data").join("main")).unwrap();
    let p = g.create_page(NewPage::titled("Rust")).unwrap();
    g.insert_block(
        p.id,
        Placement::LastChildOf(None),
        "ownership and borrowing",
    )
    .unwrap();
}

async fn rpc(
    srv: &RunningServer,
    graph: &str,
    token: Option<&str>,
    method: &str,
    params: Value,
) -> (u16, Value) {
    let mut req = reqwest::Client::new()
        .post(format!("http://{}/v1/graphs/{graph}/mcp", srv.addr))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    let body = res.text().await.unwrap();
    (status, serde_json::from_str(&body).unwrap_or(json!(body)))
}

fn init_params() -> Value {
    json!({"protocolVersion": "2025-06-18", "capabilities": {},
           "clientInfo": {"name": "test", "version": "1"}})
}

fn tool_names(v: &Value) -> Vec<String> {
    v["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{v}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_requires_its_own_token_and_an_existing_graph() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let srv = start(dir.path(), false).await;
    for token in [None, Some("wrong-token-0123456789"), Some(SYNC_TOKEN)] {
        let (status, _) = rpc(&srv, "main", token, "initialize", init_params()).await;
        assert_eq!(status, 401, "token {token:?}");
    }
    let (status, _) = rpc(&srv, "nope", Some(MCP_TOKEN), "initialize", init_params()).await;
    assert_eq!(status, 404);
    assert!(!dir.path().join("data").join("nope").exists());
    // The sync API does not accept the MCP token either.
    let res = reqwest::Client::new()
        .get(format!("http://{}/v1/graphs/main/pages", srv.addr))
        .bearer_auth(MCP_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 401);
    // DNS-rebinding guard: a foreign Host header is refused even with a valid token.
    let res = reqwest::Client::new()
        .post(format!("http://{}/v1/graphs/main/mcp", srv.addr))
        .header("host", "evil.example.com")
        .header("accept", "application/json, text/event-stream")
        .bearer_auth(MCP_TOKEN)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": init_params()}))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_client_error(), "{}", res.status());
    srv.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_server_hides_write_tools() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let srv = start(dir.path(), false).await;
    let (status, init) = rpc(&srv, "main", Some(MCP_TOKEN), "initialize", init_params()).await;
    assert_eq!(status, 200, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "synapse-mcp");
    let (_, list) = rpc(&srv, "main", Some(MCP_TOKEN), "tools/list", json!({})).await;
    let names = tool_names(&list);
    assert!(names.contains(&"search".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n == "create_page"), "{names:?}");
    let (_, r) = rpc(
        &srv,
        "main",
        Some(MCP_TOKEN),
        "tools/call",
        json!({"name": "search", "arguments": {"query": "borrowing"}}),
    )
    .await;
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("[[Rust]]"), "{r}");
    let (_, r) = rpc(
        &srv,
        "main",
        Some(MCP_TOKEN),
        "tools/call",
        json!({"name": "create_page", "arguments": {"title": "X"}}),
    )
    .await;
    assert!(
        r.get("error").is_some() || r["result"]["isError"] == json!(true),
        "{r}"
    );
    srv.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_go_through_the_op_log() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let srv = start(dir.path(), true).await;
    let (_, list) = rpc(&srv, "main", Some(MCP_TOKEN), "tools/list", json!({})).await;
    assert!(tool_names(&list).contains(&"create_page".to_string()));
    let (_, r) = rpc(
        &srv,
        "main",
        Some(MCP_TOKEN),
        "tools/call",
        json!({"name": "create_page",
               "arguments": {"title": "Borrow checker", "type": "Concept",
                             "content": "- rejects aliasing + mutation"}}),
    )
    .await;
    assert_eq!(r["result"]["isError"], json!(false), "{r}");
    // Visible through the sync API (same graph handle, logged as ops).
    let res = reqwest::Client::new()
        .get(format!(
            "http://{}/v1/graphs/main/pages/by-title?title=borrow%20checker",
            srv.addr
        ))
        .bearer_auth(SYNC_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let ops: Value = reqwest::Client::new()
        .get(format!("http://{}/v1/graphs/main/ops?since=0", srv.addr))
        .bearer_auth(SYNC_TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ops.to_string().contains("rejects aliasing"), "{ops}");
    srv.shutdown();
}
