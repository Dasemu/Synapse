//! End-to-end: a real server on a random local port and several client graphs.

use std::sync::{Arc, Mutex};
use std::time::Duration;
use synapse_core::{Graph, NewPage, Placement};
use synapse_server::{Config, RunningServer};
use synapse_sync::{BlobPolicy, Error, Remote, RemoteConfig, SyncClient, SyncConfig, SyncStatus};

const TOKEN: &str = "test-token-0123456789abcdef";

struct TestServer {
    server: RunningServer,
    _dir: tempfile::TempDir,
}

impl TestServer {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.path().join("data"),
            auth_tokens: vec![TOKEN.into()],
            update_token: None,
            update_script: "/nonexistent".into(),
            max_clock_skew: Duration::from_secs(600),
        };
        let server = synapse_server::start(&cfg).await.unwrap();
        Self { server, _dir: dir }
    }
    fn url(&self) -> String {
        format!("http://{}", self.server.addr)
    }
    fn graph_dir(&self, graph: &str) -> std::path::PathBuf {
        self._dir.path().join("data").join(graph)
    }
    fn remote_cfg(&self, graph: &str, token: &str) -> RemoteConfig {
        RemoteConfig::new(self.url(), graph, token)
    }
}

struct Device {
    graph: Arc<Mutex<Graph>>,
    client: SyncClient,
    _dir: tempfile::TempDir,
}

fn device(srv: &TestServer, graph_id: &str) -> Device {
    device_cfg(srv.remote_cfg(graph_id, TOKEN))
}

fn device_cfg(remote: RemoteConfig) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let graph = Arc::new(Mutex::new(Graph::open(dir.path()).unwrap()));
    let client = SyncClient::new(graph.clone(), SyncConfig::new(remote)).unwrap();
    Device {
        graph,
        client,
        _dir: dir,
    }
}

impl Device {
    fn g(&self) -> std::sync::MutexGuard<'_, Graph> {
        self.graph.lock().unwrap()
    }
}

fn texts(d: &Device, title: &str) -> Vec<String> {
    let g = d.g();
    let page = g.page_by_title(title).unwrap().expect("page exists");
    g.page_tree(page.id)
        .unwrap()
        .into_iter()
        .map(|(_, b)| b.content)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_devices_converge_edits_deletes_and_assets() {
    let srv = TestServer::start().await;
    let a = device(&srv, "main");
    let b = device(&srv, "main");

    // A creates a page with two blocks and an asset (multi-chunk blob).
    let (b1, b2, hash) = {
        let g = a.g();
        let p = g.create_page(NewPage::titled("Ideas")).unwrap();
        let b1 = g
            .insert_block(p.id, Placement::LastChildOf(None), "first")
            .unwrap();
        let b2 = g
            .insert_block(p.id, Placement::LastChildOf(None), "second")
            .unwrap();
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let asset = g
            .put_asset_reader(
                &data[..],
                Some("blob.bin"),
                Some("application/octet-stream"),
            )
            .unwrap();
        (b1.id, b2.id, asset.hash)
    };
    a.client.sync_once().await.unwrap();
    assert_eq!(a.client.state().pending_ops, 0);
    b.client.sync_once().await.unwrap();

    assert_eq!(texts(&b, "Ideas"), vec!["first", "second"]);
    assert!(b.g().blobs().contains(&hash), "blob downloaded by B");
    assert_eq!(b.g().asset(&hash).unwrap().unwrap().size, 3_000_000);

    // B edits one block and deletes the other; A converges.
    {
        let g = b.g();
        g.update_block(b1, "first (edited on B)").unwrap();
        g.delete_block(b2).unwrap();
    }
    assert!(b.client.refresh_pending().await.unwrap() >= 2);
    b.client.sync_once().await.unwrap();
    a.client.sync_once().await.unwrap();
    assert_eq!(texts(&a, "Ideas"), vec!["first (edited on B)"]);
    assert_eq!(texts(&b, "Ideas"), texts(&a, "Ideas"));

    // Echo avoidance: applied remote ops are never pushed back. A full extra round trip on
    // both sides must not grow the server log.
    let remote = Remote::new(srv.remote_cfg("main", TOKEN)).unwrap();
    let before = remote.ensure_graph().await.unwrap().last_seq;
    a.client.sync_once().await.unwrap();
    b.client.sync_once().await.unwrap();
    a.client.sync_once().await.unwrap();
    assert_eq!(remote.ensure_graph().await.unwrap().last_seq, before);
    assert_eq!(a.client.state().last_pushed, 0);
    assert_eq!(b.client.state().last_pushed, 0);
    assert_eq!(b.client.state().pending_ops, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_resolve_last_writer_wins() {
    let srv = TestServer::start().await;
    let a = device(&srv, "g");
    let b = device(&srv, "g");
    let blk = {
        let g = a.g();
        let p = g.create_page(NewPage::titled("Shared")).unwrap();
        g.insert_block(p.id, Placement::LastChildOf(None), "base")
            .unwrap()
            .id
    };
    a.client.sync_once().await.unwrap();
    b.client.sync_once().await.unwrap();
    {
        let g = a.g();
        g.set_now(Some(g.now_ms() + 10_000));
        g.update_block(blk, "from A").unwrap();
    }
    b.g().update_block(blk, "from B").unwrap();
    // A's edit has the later timestamp regardless of sync order.
    b.client.sync_once().await.unwrap();
    a.client.sync_once().await.unwrap();
    b.client.sync_once().await.unwrap();
    assert_eq!(texts(&a, "Shared"), vec!["from A"]);
    assert_eq!(texts(&b, "Shared"), vec!["from A"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_failures() {
    let srv = TestServer::start().await;

    let bad = device_cfg(srv.remote_cfg("main", "wrong-token-0123456789"));
    let err = bad.client.sync_once().await.unwrap_err();
    assert!(matches!(err, Error::Unauthorized), "{err:?}");
    assert_eq!(bad.client.state().status, SyncStatus::Error);

    let http = reqwest::Client::new();
    let url = srv.url();
    // Health is public; everything else needs the token.
    assert_eq!(
        http.get(format!("{url}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for path in [
        "/v1/graphs/main",
        "/v1/graphs/main/ops",
        "/v1/graphs/main/pages",
    ] {
        let r = http.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path}");
        let r = http
            .get(format!("{url}{path}"))
            .bearer_auth("nope")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{path} with wrong token");
    }
    // Update endpoint is disabled unless SYNAPSE_UPDATE_TOKEN is set.
    let r = http
        .post(format!("{url}/admin/update"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    // Bad graph id.
    let r = http
        .put(format!("{url}/v1/graphs/..%2Fetc"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    // Unknown graph on a read.
    let r = http
        .get(format!("{url}/v1/graphs/missing/ops"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn blob_upload_resumes_and_verifies_hash() {
    let srv = TestServer::start().await;
    let remote = Remote::new(srv.remote_cfg("main", TOKEN)).unwrap();
    remote.ensure_graph().await.unwrap();
    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let hash = blake3::hash(&data).to_hex().to_string();
    let url = format!("{}/v1/graphs/main/blobs/{hash}", srv.url());
    let http = reqwest::Client::new();

    // First half only.
    let r = http
        .put(&url)
        .bearer_auth(TOKEN)
        .header("upload-length", data.len())
        .body(data[..400_000].to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    assert_eq!(
        remote.blob_upload_offset(&hash).await.unwrap(),
        Some(400_000)
    );
    // Wrong offset is refused with the real one.
    let r = http
        .put(&url)
        .bearer_auth(TOKEN)
        .header("upload-length", data.len())
        .header("upload-offset", 5)
        .body(data[5..].to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(r.headers()["upload-offset"], "400000");
    // Resume through the client helper.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    std::fs::write(&path, &data).unwrap();
    remote.upload_blob(&hash, &path).await.unwrap();
    assert_eq!(remote.blob_upload_offset(&hash).await.unwrap(), None);

    // Ranged download resumes from a partial file.
    let part = dir.path().join("f.part");
    std::fs::write(&part, &data[..123_456]).unwrap();
    let dest = dir.path().join("out.bin");
    remote.download_blob(&hash, &part, &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);

    // Content that does not match its hash is rejected and not stored.
    let fake = "0".repeat(64);
    let r = http
        .put(format!("{}/v1/graphs/main/blobs/{fake}", srv.url()))
        .bearer_auth(TOKEN)
        .header("upload-length", 3)
        .body("abc")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(remote.blob_upload_offset(&fake).await.unwrap(), Some(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn thin_client_read_api() {
    let srv = TestServer::start().await;
    let a = device(&srv, "main");
    let (target, date) = {
        let g = a.g();
        let t = g.create_page(NewPage::titled("Target")).unwrap();
        g.insert_block(t.id, Placement::LastChildOf(None), "body")
            .unwrap();
        let j = g.today_journal().unwrap();
        g.insert_block(j.id, Placement::LastChildOf(None), "see [[Target]]")
            .unwrap();
        (t, j.journal_date.unwrap())
    };
    a.client.sync_once().await.unwrap();

    let thin = Remote::new(srv.remote_cfg("main", TOKEN)).unwrap();
    let v = thin
        .page_by_title("target")
        .await
        .unwrap()
        .expect("case-insensitive");
    assert_eq!(v.page.id, target.id);
    assert_eq!(v.tree.len(), 1);
    assert_eq!(v.tree[0].block.content, "body");
    assert!(thin.page_by_title("Nope").await.unwrap().is_none());
    let links = thin.backlinks(target.id).await.unwrap();
    assert_eq!(links.len(), 1);
    assert!(links[0].block.content.contains("Target"));
    let j = thin.journal(date).await.unwrap().expect("journal");
    assert_eq!(j.tree[0].block.content, "see [[Target]]");
    assert!(
        thin.journal(chrono::NaiveDate::from_ymd_opt(1999, 1, 1).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(thin.list_pages(true, 50, 0).await.unwrap().len() >= 2);
    assert_eq!(thin.journals_between(date, date).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_keeps_edits_and_reports_state() {
    // Nothing listens on this port.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let d = device_cfg(RemoteConfig::new(
        format!("http://127.0.0.1:{port}"),
        "main",
        TOKEN,
    ));
    {
        let g = d.g();
        let p = g.create_page(NewPage::titled("Offline note")).unwrap();
        g.insert_block(p.id, Placement::LastChildOf(None), "kept")
            .unwrap();
    }
    assert!(d.client.sync_once().await.is_err());
    let st = d.client.state();
    assert_eq!(st.status, SyncStatus::Offline);
    assert!(st.pending_ops >= 2, "{st:?}");
    assert_eq!(texts(&d, "Offline note"), vec!["kept"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn background_loop_delivers_realtime_via_sse() {
    let srv = TestServer::start().await;
    let mk = |id: &str| {
        let dir = tempfile::tempdir().unwrap();
        let graph = Arc::new(Mutex::new(Graph::open(dir.path()).unwrap()));
        let mut cfg = SyncConfig::new(srv.remote_cfg(id, TOKEN));
        // Polling is effectively off: B can only learn about A's edit through the stream.
        cfg.poll_interval = Duration::from_secs(3600);
        cfg.full_sync_interval = Duration::from_secs(3600);
        cfg.blob_policy = BlobPolicy::OnDemand;
        (SyncClient::new(graph.clone(), cfg).unwrap(), graph, dir)
    };
    let (ca, ga, _da) = mk("rt");
    let (cb, gb, _db) = mk("rt");
    let _ha = ca.start();
    let _hb = cb.start();
    tokio::time::sleep(Duration::from_millis(800)).await; // let both connect to the stream

    {
        let g = ga.lock().unwrap();
        let p = g.create_page(NewPage::titled("Live")).unwrap();
        g.insert_block(p.id, Placement::LastChildOf(None), "hello")
            .unwrap();
    }
    ca.trigger();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if gb.lock().unwrap().page_by_title("Live").unwrap().is_some() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "B never received the page"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn put_asset(d: &Device, len: u32) -> String {
    let data: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
    d.g()
        .put_asset_reader(&data[..], Some("a.bin"), None)
        .unwrap()
        .hash
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_local_blob_does_not_block_later_ops() {
    let srv = TestServer::start().await;
    let a = device(&srv, "main");
    let hash = put_asset(&a, 10_000);
    std::fs::remove_file(a.g().blobs().path_of(&hash)).unwrap();
    {
        let g = a.g();
        let p = g.create_page(NewPage::titled("After")).unwrap();
        g.insert_block(p.id, Placement::LastChildOf(None), "still pushed")
            .unwrap();
    }
    a.client.sync_once().await.expect("cycle succeeds");
    let st = a.client.state();
    assert_eq!(st.status, SyncStatus::Idle);
    assert_eq!(st.skipped_assets, 1);
    assert_eq!(st.pending_ops, 0);
    let thin = Remote::new(srv.remote_cfg("main", TOKEN)).unwrap();
    assert!(thin.page_by_title("After").await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_remote_blob_does_not_wedge_the_cycle() {
    let srv = TestServer::start().await;
    let a = device(&srv, "main");
    let b = device(&srv, "main");
    let hash = put_asset(&a, 10_000);
    {
        let g = a.g();
        let p = g.create_page(NewPage::titled("Doc")).unwrap();
        g.insert_block(p.id, Placement::LastChildOf(None), "text")
            .unwrap();
    }
    a.client.sync_once().await.unwrap();
    // The server loses the blob (disk cleanup, manual deletion...).
    let blob = srv
        .graph_dir("main")
        .join("blobs")
        .join(&hash[..2])
        .join(&hash[2..]);
    std::fs::remove_file(blob).unwrap();
    b.client
        .sync_once()
        .await
        .expect("cycle succeeds despite the 404");
    let st = b.client.state();
    assert_eq!(st.status, SyncStatus::Idle);
    assert_eq!(st.failed_blobs, 1);
    assert_eq!(texts(&b, "Doc"), vec!["text"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn server_rejects_bad_ops_but_keeps_the_rest() {
    use synapse_sync::protocol::WireOp;
    let srv = TestServer::start().await;
    let remote = Remote::new(srv.remote_cfg("main", TOKEN)).unwrap();
    remote.ensure_graph().await.unwrap();
    let id = synapse_core::new_id();
    let op = synapse_core::Op::DeleteBlock { id };
    let bad_asset = synapse_core::Op::PutAsset {
        asset: synapse_core::Asset {
            hash: "../../etc/passwd".into(),
            size: 1,
            mime: None,
            original_name: None,
            created_at: 0,
        },
    };
    let ok_hlc = format!("{:013}-00000-peer", chrono::Utc::now().timestamp_millis());
    let far = chrono::Utc::now().timestamp_millis() + 3_600_000;
    let resp = remote
        .push(vec![
            WireOp {
                hlc: ok_hlc.clone(),
                op: op.clone(),
            },
            WireOp {
                hlc: ok_hlc.clone(),
                op: bad_asset,
            },
            WireOp {
                hlc: format!("{far:013}-00000-peer"),
                op: op.clone(),
            },
            WireOp {
                hlc: "garbage".into(),
                op,
            },
        ])
        .await
        .unwrap();
    assert_eq!(resp.applied, 1);
    let idx: Vec<u32> = resp.rejected.iter().map(|r| r.index).collect();
    assert_eq!(idx, vec![1, 2, 3], "{:?}", resp.rejected);
    assert!(resp.rejected[1].reason.contains("future"));
}

#[tokio::test(flavor = "multi_thread")]
async fn poison_op_in_server_log_does_not_wedge_pull() {
    let srv = TestServer::start().await;
    let a = device(&srv, "main");
    a.client.sync_once().await.unwrap(); // creates the server graph
    // Plant ops straight into the server log, bypassing push validation: an asset with a
    // hostile hash (core may reject it as `Invalid`, which the client quarantines; older core
    // applies it, which the client must then ignore) followed by a normal page.
    {
        let g = Graph::open(srv.graph_dir("main")).unwrap();
        let bad = serde_json::json!({"type": "put_asset", "asset": {
            "hash": "../../evil", "size": 1, "mime": null, "original_name": null, "created_at": 0}});
        let ms = chrono::Utc::now().timestamp_millis();
        g.conn()
            .execute(
                "INSERT INTO ops(hlc, payload) VALUES (?1, ?2)",
                [
                    format!("{ms:013}-00000-other").as_str(),
                    bad.to_string().as_str(),
                ],
            )
            .unwrap();
    }
    let c = device(&srv, "main");
    {
        let g = c.g();
        let p = g.create_page(NewPage::titled("After poison")).unwrap();
        g.insert_block(p.id, Placement::LastChildOf(None), "x")
            .unwrap();
    }
    c.client.sync_once().await.unwrap();
    let b = device(&srv, "main");
    b.client.sync_once().await.expect("pull is not wedged");
    assert_eq!(b.client.state().status, SyncStatus::Idle);
    assert_eq!(texts(&b, "After poison"), vec!["x"]);
}
