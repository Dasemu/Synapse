use crate::error::{Error, Result};
use crate::protocol::{SyncEvent, WireOp, hlc_actor, is_poison};
use crate::remote::{Remote, RemoteConfig};
use crate::state::{self, Cursors, SyncState, SyncStatus};
use futures_util::StreamExt;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use synapse_core::{Graph, Op};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobPolicy {
    /// Download blobs of synced assets right after pulling their ops.
    Eager,
    /// Only download when asked via [`SyncClient::ensure_blob`] (e.g. metered connections).
    OnDemand,
}

#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub remote: RemoteConfig,
    /// Ops per push/pull request.
    pub batch_size: u32,
    pub blob_policy: BlobPolicy,
    /// Background loop: how often to check for unpushed local ops.
    pub poll_interval: Duration,
    /// Background loop: full sync at least this often even without notifications.
    pub full_sync_interval: Duration,
    pub max_backoff: Duration,
}

impl SyncConfig {
    pub fn new(remote: RemoteConfig) -> Self {
        Self {
            remote,
            batch_size: 500,
            blob_policy: BlobPolicy::Eager,
            poll_interval: Duration::from_secs(5),
            full_sync_interval: Duration::from_secs(120),
            max_backoff: Duration::from_secs(300),
        }
    }
}

struct Inner {
    graph: Arc<Mutex<Graph>>,
    cfg: SyncConfig,
    remote: Remote,
    state: watch::Sender<SyncState>,
    /// Serialises sync cycles (manual `sync_once` vs the background loop).
    cycle: tokio::sync::Mutex<()>,
    /// Remote changed (SSE) or caller asked for a sync.
    dirty: AtomicBool,
    wake: Notify,
    /// Some synced assets may lack their blob locally.
    blobs_dirty: AtomicBool,
    quarantined: AtomicU64,
}

/// Per-cycle counters.
#[derive(Debug, Default)]
struct Cycle {
    pushed: u64,
    pulled: u64,
    skipped_assets: u64,
    failed_blobs: u64,
}

/// Replicates a local [`Graph`] with a Synapse server. Cheap to clone.
///
/// The graph is shared as `Arc<Mutex<Graph>>` (the core `Graph` is not `Sync`); the client only
/// holds the lock for short database steps, never across network calls.
#[derive(Clone)]
pub struct SyncClient {
    inner: Arc<Inner>,
}

impl SyncClient {
    pub fn new(graph: Arc<Mutex<Graph>>, cfg: SyncConfig) -> Result<Self> {
        let remote = Remote::new(cfg.remote.clone())?;
        let (state, _) = watch::channel(SyncState::default());
        let inner = Arc::new(Inner {
            graph,
            cfg,
            remote,
            state,
            cycle: tokio::sync::Mutex::new(()),
            dirty: AtomicBool::new(true),
            wake: Notify::new(),
            blobs_dirty: AtomicBool::new(true),
            quarantined: AtomicU64::new(0),
        });
        Ok(Self { inner })
    }

    pub fn remote(&self) -> &Remote {
        &self.inner.remote
    }

    /// Current state for the UI.
    pub fn state(&self) -> SyncState {
        self.inner.state.borrow().clone()
    }

    /// Receiver that fires on every state change.
    pub fn subscribe(&self) -> watch::Receiver<SyncState> {
        self.inner.state.subscribe()
    }

    /// Recomputes the pending-ops count (call after local edits to update the UI badge).
    pub async fn refresh_pending(&self) -> Result<u64> {
        let n = self.with_graph(|g| Ok(state::pending_now(g)?)).await?;
        self.inner.state.send_modify(|s| s.pending_ops = n);
        Ok(n)
    }

    /// Asks the background loop to sync soon (call after local edits).
    pub fn trigger(&self) {
        self.inner.dirty.store(true, Ordering::SeqCst);
        self.inner.wake.notify_one();
    }

    async fn with_graph<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Graph) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let graph = self.inner.graph.clone();
        tokio::task::spawn_blocking(move || {
            let g = graph.lock().unwrap_or_else(PoisonError::into_inner);
            f(&g)
        })
        .await
        .map_err(|e| Error::Task(e.to_string()))?
    }

    /// One full cycle: push local ops (+ their blobs), pull remote ops, fetch blobs.
    pub async fn sync_once(&self) -> Result<()> {
        let _cycle = self.inner.cycle.lock().await;
        self.inner.dirty.store(false, Ordering::SeqCst);
        self.inner.state.send_modify(|s| {
            s.status = SyncStatus::Syncing;
            s.message = None;
        });
        let res = self.run_cycle().await;
        let pending = self
            .with_graph(|g| Ok(state::pending_now(g)?))
            .await
            .unwrap_or(0);
        match &res {
            Ok(c) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                self.inner.state.send_modify(|s| {
                    s.status = SyncStatus::Idle;
                    s.message = None;
                    s.pending_ops = pending;
                    s.last_sync_ms = Some(now);
                    s.last_pushed = c.pushed;
                    s.last_pulled = c.pulled;
                    s.skipped_assets = c.skipped_assets;
                    s.failed_blobs = c.failed_blobs;
                    s.quarantined_ops = self.inner.quarantined.load(Ordering::SeqCst);
                });
            }
            Err(e) => {
                let status = if e.is_offline() {
                    SyncStatus::Offline
                } else {
                    SyncStatus::Error
                };
                warn!(error = %e, ?status, "sync cycle failed");
                self.inner.state.send_modify(|s| {
                    s.status = status;
                    s.message = Some(e.to_string());
                    s.pending_ops = pending;
                    s.quarantined_ops = self.inner.quarantined.load(Ordering::SeqCst);
                });
            }
        }
        res.map(|_| ())
    }

    async fn run_cycle(&self) -> Result<Cycle> {
        let remote = &self.inner.remote;
        let info = remote.ensure_graph().await?;
        let key = remote.key();
        let server_id = info.server_id.clone();
        let mut cur = self
            .with_graph(move |g| Ok(state::load(g, &key, &server_id)?))
            .await?;

        let mut stats = Cycle::default();
        self.push_all(&mut cur, &mut stats).await?;
        let (pulled, got_assets) = self.pull_all(&mut cur, &info.server_id).await?;
        stats.pulled = pulled;
        if got_assets {
            self.inner.blobs_dirty.store(true, Ordering::SeqCst);
        }
        if self.inner.cfg.blob_policy == BlobPolicy::Eager
            && self.inner.blobs_dirty.load(Ordering::SeqCst)
        {
            stats.failed_blobs = self.fetch_missing_blobs().await?;
            // Failed blobs are retried on the next cycle.
            self.inner
                .blobs_dirty
                .store(stats.failed_blobs > 0, Ordering::SeqCst);
        }
        Ok(stats)
    }

    /// Pushes ops originated by this replica. Ops that were *applied from the server* are
    /// re-logged locally by the core, but carry another actor in their HLC, so they are skipped
    /// here: that is what prevents echoing them back.
    async fn push_all(&self, cur: &mut Cursors, stats: &mut Cycle) -> Result<()> {
        let limit = self.inner.cfg.batch_size;
        loop {
            let after = cur.pushed_seq;
            let (batch, scanned_to, scanned) = self
                .with_graph(move |g| {
                    let recs = g.ops_since(after, limit)?;
                    let n = recs.len();
                    let last = recs.last().map_or(after, |r| r.seq);
                    let me = g.actor().to_string();
                    let ops: Vec<WireOp> = recs
                        .into_iter()
                        .filter(|r| hlc_actor(&r.hlc) == Some(me.as_str()))
                        .map(|r| WireOp {
                            hlc: r.hlc,
                            op: r.op,
                        })
                        .collect();
                    Ok((ops, last, n))
                })
                .await?;
            if scanned == 0 {
                return Ok(());
            }
            let mut batch = batch;
            if !batch.is_empty() {
                // Blobs first, so the server never knows an asset it cannot serve. An asset
                // whose blob is missing locally can never be uploaded: skip its op (and warn)
                // instead of blocking every later op of this device.
                let hashes: BTreeSet<String> = batch
                    .iter()
                    .filter_map(|w| match &w.op {
                        Op::PutAsset { asset } => Some(asset.hash.clone()),
                        _ => None,
                    })
                    .collect();
                let mut missing = BTreeSet::new();
                for h in hashes {
                    if !self.upload_blob(&h).await? {
                        missing.insert(h);
                    }
                }
                if !missing.is_empty() {
                    let before = batch.len();
                    batch.retain(|w| match &w.op {
                        Op::PutAsset { asset } => !missing.contains(&asset.hash),
                        _ => true,
                    });
                    stats.skipped_assets += (before - batch.len()) as u64;
                }
                if !batch.is_empty() {
                    stats.pushed += batch.len() as u64;
                    let resp = self.inner.remote.push(batch).await?;
                    if !resp.rejected.is_empty() {
                        self.inner
                            .quarantined
                            .fetch_add(resp.rejected.len() as u64, Ordering::SeqCst);
                        for r in &resp.rejected {
                            warn!(index = r.index, reason = %r.reason, "server rejected a local op");
                        }
                    }
                }
            }
            self.with_graph(move |g| Ok(state::save_pushed(g, scanned_to)?))
                .await?;
            cur.pushed_seq = scanned_to;
            if (scanned as u32) < limit {
                return Ok(());
            }
        }
    }

    async fn pull_all(&self, cur: &mut Cursors, server_id: &str) -> Result<(u64, bool)> {
        let limit = self.inner.cfg.batch_size;
        let me = self.with_graph(|g| Ok(g.actor().to_string())).await?;
        let mut applied_total = 0;
        let mut got_assets = false;
        loop {
            let resp = self
                .inner
                .remote
                .pull(cur.pull_cursor, limit, Some(&me))
                .await?;
            if resp.server_id != server_id {
                return Err(Error::Protocol("server graph changed mid-sync".into()));
            }
            let next = resp.next_cursor.max(cur.pull_cursor);
            let me2 = me.clone();
            let (applied, assets, quarantined) = self
                .with_graph(move |g| {
                    let (mut applied, mut assets, mut quarantined) = (0u64, false, 0u64);
                    for rec in resp.ops {
                        // Defence in depth: never re-apply our own ops.
                        if hlc_actor(&rec.hlc) == Some(me2.as_str()) {
                            continue;
                        }
                        match g.apply_remote(&rec.hlc, &rec.op) {
                            Ok(true) => {
                                applied += 1;
                                assets |= matches!(rec.op, Op::PutAsset { .. });
                            }
                            Ok(false) => {}
                            // An op that can never apply must not wedge the pull forever:
                            // park it and move on. Storage errors stay fatal (retried).
                            Err(e) if is_poison(&e) => {
                                warn!(seq = rec.seq, hlc = %rec.hlc, error = %e, "quarantined remote op");
                                let payload = serde_json::to_string(&rec.op)
                                    .map_err(synapse_core::Error::from)?;
                                state::quarantine(g, rec.seq, &rec.hlc, &payload, &e.to_string())?;
                                quarantined += 1;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                    state::save_pulled(g, next)?;
                    Ok((applied, assets, quarantined))
                })
                .await?;
            self.inner
                .quarantined
                .fetch_add(quarantined, Ordering::SeqCst);
            cur.pull_cursor = next;
            applied_total += applied;
            got_assets |= assets;
            if !resp.has_more {
                return Ok((applied_total, got_assets));
            }
        }
    }

    // ----------------------------------------------------------------- blobs

    /// Uploads the blob of a local asset. `Ok(false)` if the blob is missing locally.
    async fn upload_blob(&self, hash: &str) -> Result<bool> {
        let h = hash.to_string();
        let path = self.with_graph(move |g| Ok(g.blobs().path_of(&h))).await?;
        if !path.exists() {
            warn!(%hash, "asset registered but its blob is missing locally; not pushing it");
            return Ok(false);
        }
        self.inner.remote.upload_blob(hash, &path).await?;
        Ok(true)
    }

    /// Downloads blob `hash` if it is not stored locally yet (resumes partial downloads).
    pub async fn ensure_blob(&self, hash: &str) -> Result<()> {
        if !synapse_core_hash_ok(hash) {
            return Err(Error::Protocol(format!("invalid blob hash {hash:?}")));
        }
        let h = hash.to_string();
        let (dest, part, have) = self
            .with_graph(move |g| {
                let dest = g.blobs().path_of(&h);
                let part = g
                    .dir()
                    .join("blobs")
                    .join("tmp")
                    .join(format!("sync-{h}.part"));
                Ok((dest.clone(), part, dest.exists()))
            })
            .await?;
        if have {
            return Ok(());
        }
        let mut last = None;
        for _ in 0..2 {
            match self.inner.remote.download_blob(hash, &part, &dest).await {
                Ok(()) => return Ok(()),
                Err(e @ Error::Protocol(_)) => last = Some(e), // retry from scratch
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap())
    }

    /// Downloads every asset blob we lack. A blob that fails (e.g. 404) is logged and left
    /// for the next cycle; the others still download. Returns how many failed. Connectivity
    /// errors abort the cycle since every other blob would fail too.
    async fn fetch_missing_blobs(&self) -> Result<u64> {
        let missing = self
            .with_graph(|g| {
                let mut st = g.conn().prepare("SELECT hash FROM assets")?;
                let hashes = st
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(hashes
                    .into_iter()
                    .filter(|h| {
                        let valid = crate::protocol::is_valid_hash(h);
                        if !valid {
                            warn!(hash = %h, "ignoring asset with an invalid blob hash");
                        }
                        valid && !g.blobs().contains(h)
                    })
                    .collect::<Vec<_>>())
            })
            .await?;
        let mut failed = 0;
        for h in missing {
            debug!(hash = %h, "downloading blob");
            match self.ensure_blob(&h).await {
                Ok(()) => {}
                Err(e) if e.is_offline() || matches!(e, Error::Unauthorized) => return Err(e),
                Err(e) => {
                    warn!(hash = %h, error = %e, "blob download failed; will retry next cycle");
                    failed += 1;
                }
            }
        }
        Ok(failed)
    }

    // ------------------------------------------------------------ background

    /// Starts the background loop on the current tokio runtime: syncs on server notifications
    /// (SSE), when local ops are pending, and periodically; backs off exponentially on errors
    /// and keeps working offline. Stop it with [`SyncHandle::stop`] or by dropping the handle.
    pub fn start(&self) -> SyncHandle {
        let me = self.clone();
        let loop_task = tokio::spawn(async move { me.run_loop().await });
        let me = self.clone();
        let events_task = tokio::spawn(async move { me.run_events().await });
        SyncHandle {
            client: self.clone(),
            tasks: vec![loop_task, events_task],
        }
    }

    async fn run_loop(self) {
        let cfg = &self.inner.cfg;
        let mut backoff = Duration::from_secs(1);
        let mut last_full: Option<tokio::time::Instant> = None;
        loop {
            let pending = self.refresh_pending().await.unwrap_or(0);
            let due = last_full.is_none_or(|t| t.elapsed() >= cfg.full_sync_interval);
            if self.inner.dirty.load(Ordering::SeqCst) || pending > 0 || due {
                match self.sync_once().await {
                    Ok(()) => {
                        backoff = Duration::from_secs(1);
                        last_full = Some(tokio::time::Instant::now());
                    }
                    Err(e) => {
                        let wait = jitter(backoff);
                        debug!(error = %e, ?wait, "sync failed; backing off");
                        // Keep `dirty` so we retry as soon as the backoff elapses.
                        self.inner.dirty.store(true, Ordering::SeqCst);
                        backoff = (backoff * 2).min(cfg.max_backoff);
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                }
            }
            tokio::select! {
                _ = self.inner.wake.notified() => {}
                _ = tokio::time::sleep(cfg.poll_interval) => {}
            }
        }
    }

    async fn run_events(self) {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.inner.remote.open_events().await {
                Ok(resp) => {
                    backoff = Duration::from_secs(1);
                    info!("realtime stream connected");
                    // We may have missed events while disconnected.
                    self.trigger();
                    let mut buf = String::new();
                    let mut stream = resp.bytes_stream();
                    while let Some(chunk) = stream.next().await {
                        let Ok(chunk) = chunk else { break };
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(i) = buf.find("\n\n") {
                            let event: String = buf.drain(..i + 2).collect();
                            let notified = event
                                .lines()
                                .filter_map(|l| l.strip_prefix("data:"))
                                .any(|d| serde_json::from_str::<SyncEvent>(d.trim()).is_ok());
                            if notified {
                                self.trigger();
                            }
                        }
                    }
                }
                Err(e) => debug!(error = %e, "realtime stream unavailable"),
            }
            tokio::time::sleep(jitter(backoff)).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    }
}

/// Handle of the background sync loop. Dropping it stops the loop.
pub struct SyncHandle {
    client: SyncClient,
    tasks: Vec<JoinHandle<()>>,
}

impl SyncHandle {
    pub fn client(&self) -> &SyncClient {
        &self.client
    }

    pub fn trigger(&self) {
        self.client.trigger();
    }

    pub fn stop(self) {}
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    d + d.mul_f64((nanos % 1000) as f64 / 4000.0)
}

fn synapse_core_hash_ok(h: &str) -> bool {
    crate::protocol::is_valid_hash(h)
}
