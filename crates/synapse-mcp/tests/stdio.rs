//! Smoke test of the `synapse-mcp` binary over stdio: initialize, tools/list, tools/call.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use synapse_core::{Graph, NewPage, Placement};

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(graph: &std::path::Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
            .arg("--graph")
            .arg(graph)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The response to request `id` (skipping notifications).
    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("server reply");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }

    fn init(&mut self) -> Value {
        let r = self.call(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "1"}}),
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        r
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tool_names(list: &Value) -> Vec<String> {
    list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn stdio_initialize_list_and_call() {
    let dir = tempfile::tempdir().unwrap();
    {
        let g = Graph::open(dir.path()).unwrap();
        let p = g.create_page(NewPage::titled("Rust")).unwrap();
        g.insert_block(
            p.id,
            Placement::LastChildOf(None),
            "ownership and borrowing",
        )
        .unwrap();
    }
    let mut s = Server::start(dir.path(), &[]);
    let init = s.init();
    assert_eq!(
        init["result"]["serverInfo"]["name"], "synapse-mcp",
        "{init}"
    );
    let names = tool_names(&s.call(2, "tools/list", json!({})));
    assert!(names.contains(&"search".to_string()), "{names:?}");
    assert!(
        !names.contains(&"create_page".to_string()),
        "read-only: {names:?}"
    );
    let r = s.call(
        3,
        "tools/call",
        json!({"name": "search", "arguments": {"query": "borrowing"}}),
    );
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("[[Rust]]"), "{r}");
    // Write tools are not callable read-only.
    let r = s.call(
        4,
        "tools/call",
        json!({"name": "create_page", "arguments": {"title": "X"}}),
    );
    assert!(
        r.get("error").is_some() || r["result"]["isError"] == json!(true),
        "{r}"
    );
    drop(s);

    let mut s = Server::start(dir.path(), &["--allow-write"]);
    s.init();
    let names = tool_names(&s.call(2, "tools/list", json!({})));
    assert!(names.contains(&"create_page".to_string()), "{names:?}");
    let r = s.call(
        3,
        "tools/call",
        json!({"name": "create_page", "arguments": {"title": "Borrow checker", "type": "Concept"}}),
    );
    assert_eq!(r["result"]["isError"], json!(false), "{r}");
    drop(s);
    let g = Graph::open(dir.path()).unwrap();
    let p = g.page_by_title("borrow checker").unwrap().unwrap();
    assert_eq!(p.page_type(), Some("Concept"));
}

#[test]
fn refuses_a_missing_graph() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
        .arg("--graph")
        .arg(dir.path().join("nope"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!dir.path().join("nope").exists(), "must not create a graph");
}
