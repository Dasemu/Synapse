//! Pushes to a real `git receive-pack` behind a tiny smart-HTTP shim.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use synapse_core::{Graph, Placement};
use synapse_history::{History, HistoryOptions, PushAuth};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn serve(
    remote: std::path::PathBuf,
    requests: usize,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let mut auths = Vec::new();
        for _ in 0..requests {
            let (mut s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let is_get = line.starts_with("GET");
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                let h = h.trim_end().to_string();
                if h.is_empty() {
                    break;
                }
                let lower = h.to_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if lower.starts_with("authorization:") {
                    auths.push(h.clone());
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let mut cmd = Command::new("git");
            cmd.arg("receive-pack").arg("--stateless-rpc");
            if is_get {
                cmd.arg("--advertise-refs");
            }
            let mut child = cmd
                .arg(&remote)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&body).unwrap();
            let out = child.wait_with_output().unwrap().stdout;
            let mut payload = Vec::new();
            let ct = if is_get {
                payload.extend_from_slice(b"001f# service=git-receive-pack\n0000");
                "application/x-git-receive-pack-advertisement"
            } else {
                "application/x-git-receive-pack-result"
            };
            payload.extend_from_slice(&out);
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).unwrap();
            s.write_all(&payload).unwrap();
        }
        auths
    });
    (url, h)
}

#[test]
fn push_incrementally_to_a_real_git_remote() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed, skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    g.set_now(Some(1_700_000_000_000));
    let mut h = History::open(dir.path(), HistoryOptions::default()).unwrap();
    let p = g.ensure_page("Pushed", None).unwrap();
    g.insert_block(p.id, Placement::LastChildOf(None), "hello remote")
        .unwrap();
    h.snapshot(&g, 1_700_000_000_000).unwrap();

    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q", "."]);
    let auth = PushAuth {
        username: "u".into(),
        secret: "tok".into(),
    };

    let (url, srv) = serve(remote.path().into(), 2);
    let r = h.push(&url, "main", Some(&auth), false).unwrap();
    assert!(r.updated && r.objects > 3);
    assert_eq!(
        git(remote.path(), &["show", "main:pages/Pushed.md"]),
        "- hello remote\n"
    );
    git(remote.path(), &["fsck", "--strict"]);
    let auths = srv.join().unwrap();
    assert!(
        auths.iter().all(|a| a.contains("Basic dTp0b2s=")),
        "{auths:?}"
    );

    // Second push: new snapshot only sends the delta; unchanged tip is a no-op.
    g.set_now(Some(1_700_000_100_000));
    g.insert_block(p.id, Placement::LastChildOf(None), "more")
        .unwrap();
    h.snapshot(&g, 1_700_000_100_000).unwrap();
    let (url, srv) = serve(remote.path().into(), 2);
    let r = h.push(&url, "main", Some(&auth), false).unwrap();
    assert!(r.updated && r.objects < 8, "{r:?}");
    srv.join().unwrap();
    assert_eq!(
        git(remote.path(), &["rev-list", "--count", "main"]).trim(),
        "2"
    );
    git(remote.path(), &["fsck", "--strict"]);

    let (url, srv) = serve(remote.path().into(), 1);
    assert!(!h.push(&url, "main", Some(&auth), false).unwrap().updated);
    srv.join().unwrap();
}
