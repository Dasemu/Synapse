//! `synapse-mcp`: MCP server over stdio for a local Synapse graph.
//!
//! Usage: `synapse-mcp [--graph <dir>] [--allow-write]`. The graph defaults to
//! `$SYNAPSE_GRAPH`, else the app's own location (`<app support dir>/graph`).

use rmcp::ServiceExt;
use std::path::PathBuf;
use std::sync::Arc;
use synapse_mcp::{LocalHost, SynapseMcp};

const USAGE: &str = "usage: synapse-mcp [--graph <dir>] [--allow-write]

MCP server (stdio) for a local Synapse graph. Read-only unless --allow-write.
The graph defaults to $SYNAPSE_GRAPH, else the app's directory:
  Linux:   $XDG_DATA_HOME/dev.dasemu.synapse/graph (~/.local/share/...)
  Windows: %APPDATA%\\dasemu\\Synapse\\graph";

/// The directory the Flutter app uses (`getApplicationSupportDirectory()/graph`).
fn default_graph() -> Option<PathBuf> {
    if let Some(g) = std::env::var_os("SYNAPSE_GRAPH").filter(|v| !v.is_empty()) {
        return Some(g.into());
    }
    if cfg!(windows) {
        let appdata = std::env::var_os("APPDATA")?;
        return Some(
            PathBuf::from(appdata)
                .join("dasemu")
                .join("Synapse")
                .join("graph"),
        );
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("dev.dasemu.synapse").join("graph"))
}

fn fail(msg: &str) -> ! {
    eprintln!("synapse-mcp: {msg}");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let mut graph: Option<PathBuf> = None;
    let mut allow_write = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--graph" => graph = Some(args.next().unwrap_or_else(|| fail(USAGE)).into()),
            "--allow-write" => allow_write = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => fail(USAGE),
        }
    }
    let dir = graph
        .or_else(default_graph)
        .unwrap_or_else(|| fail("no graph directory; pass --graph <dir>"));
    let host = LocalHost::new(&dir).unwrap_or_else(|e| fail(&e.to_string()));
    eprintln!(
        "synapse-mcp: serving {} ({})",
        dir.display(),
        if allow_write {
            "read-write"
        } else {
            "read-only"
        }
    );
    let server = SynapseMcp::new(Some(Arc::new(host)), allow_write)
        .serve(rmcp::transport::stdio())
        .await
        .unwrap_or_else(|e| fail(&format!("startup failed: {e}")));
    if let Err(e) = server.waiting().await {
        fail(&format!("server error: {e}"));
    }
}
