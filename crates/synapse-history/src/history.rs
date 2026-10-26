//! Snapshot engine: incremental Markdown export committed to a bare git repository.

use crate::Result;
use crate::error::Error;
use crate::export::{self, JOURNALS_DIR, MANIFEST, PAGES_DIR, TYPES_FILE};
use crate::repo::{CommitInfo, Files, Repo};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use synapse_core::{BlockId, Graph, Op, Page, PageId};

const STATE_FILE: &str = "synapse-history.json";
const OPS_BATCH: u32 = 2000;
const TITLES_IN_MESSAGE: usize = 5;

/// When [`History::maybe_snapshot`] actually commits.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotPolicy {
    /// Never snapshot more often than this.
    pub min_interval: Duration,
    /// Snapshot at the latest this long after the previous one if anything changed.
    pub max_interval: Duration,
    /// Snapshot once the graph has been quiet this long (still respecting `min_interval`).
    pub on_idle: Option<Duration>,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(5 * 60),
            max_interval: Duration::from_secs(60 * 60),
            on_idle: Some(Duration::from_secs(2 * 60)),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HistoryOptions {
    pub policy: SnapshotPolicy,
    /// Also commit asset bytes (`blobs/<hash>`). Off by default: blobs are already
    /// content-addressed and only the manifest is versioned.
    pub include_blobs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// Full commit id (hex).
    pub id: String,
    /// Seconds since epoch.
    pub time: i64,
    /// First line of the commit message.
    pub summary: String,
}

impl From<CommitInfo> for SnapshotInfo {
    fn from(c: CommitInfo) -> Self {
        Self {
            id: c.id,
            time: c.time,
            summary: c.message.lines().next().unwrap_or("").to_string(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// Last op seq covered by the newest snapshot.
    last_seq: i64,
    /// Graph clock (ms) when the newest snapshot was taken.
    snapshot_ms: i64,
    /// Blocks that carried an `id::` property in the newest snapshot.
    referenced: BTreeSet<BlockId>,
}

pub struct History {
    pub(crate) repo: Repo,
    dir: PathBuf,
    opts: HistoryOptions,
    /// `(last seen graph seq, time we saw it change)` for idle detection.
    observed: Option<(i64, i64)>,
    pub(crate) fault: std::cell::Cell<Option<usize>>,
}

impl History {
    /// Opens (creating if needed) `<graph_dir>/history/`.
    pub fn open(graph_dir: impl AsRef<Path>, opts: HistoryOptions) -> Result<Self> {
        Self::open_at(graph_dir.as_ref().join("history"), opts)
    }

    pub fn open_at(dir: impl Into<PathBuf>, opts: HistoryOptions) -> Result<Self> {
        let dir = dir.into();
        let repo = Repo::open_or_init(&dir)?;
        Ok(Self {
            repo,
            dir,
            opts,
            observed: None,
            fault: Default::default(),
        })
    }

    /// Test hook: make the next restore fail after `n` block writes/deletes.
    #[doc(hidden)]
    pub fn inject_restore_fault(&self, n: Option<usize>) {
        self.fault.set(n);
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn load_state(&self) -> Option<State> {
        serde_json::from_slice(&std::fs::read(self.dir.join(STATE_FILE)).ok()?).ok()
    }

    fn save_state(&self, s: &State) -> Result<()> {
        let tmp = self.dir.join(format!("{STATE_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(s)?)?;
        std::fs::rename(tmp, self.dir.join(STATE_FILE))?;
        Ok(())
    }

    /// True when the graph has changes not yet covered by a snapshot.
    pub fn has_pending(&self, graph: &Graph) -> Result<bool> {
        Ok(match self.load_state() {
            Some(s) => graph.last_seq()? != s.last_seq,
            None => true,
        })
    }

    /// Called periodically by the app; snapshots when the policy says so.
    /// `now_ms` is wall-clock milliseconds.
    pub fn maybe_snapshot(&mut self, graph: &Graph, now_ms: i64) -> Result<Option<SnapshotInfo>> {
        let seq = graph.last_seq()?;
        let changed_at = match self.observed {
            Some((s, t)) if s == seq => t,
            _ => now_ms,
        };
        self.observed = Some((seq, changed_at));
        let state = self.load_state();
        if state.as_ref().is_some_and(|s| s.last_seq == seq) {
            return Ok(None);
        }
        let p = self.opts.policy;
        let ms = |d: Duration| d.as_millis() as i64;
        let since_last = match (&state, self.repo.head()?) {
            (Some(_), Some(head)) => now_ms - self.repo.commit_info(head)?.time * 1000,
            _ => i64::MAX, // first snapshot: take it as soon as there is something
        };
        if since_last < ms(p.min_interval) {
            return Ok(None);
        }
        let idle = p.on_idle.is_some_and(|d| now_ms - changed_at >= ms(d));
        if since_last >= ms(p.max_interval) || idle {
            return self.snapshot(graph, now_ms);
        }
        Ok(None)
    }

    /// Takes a snapshot now. Returns `None` when nothing changed.
    pub fn snapshot(&mut self, graph: &Graph, now_ms: i64) -> Result<Option<SnapshotInfo>> {
        let state = self.load_state();
        let head = self.repo.head()?;
        let seq_now = graph.last_seq()?;
        let full = match (&state, head) {
            (Some(s), Some(_)) => s.last_seq > seq_now,
            _ => true,
        };
        if !full && state.as_ref().is_some_and(|s| s.last_seq == seq_now) {
            return Ok(None);
        }
        let old_state = state.unwrap_or_default();
        let old_files = match head {
            Some(h) => self.repo.files(h)?,
            None => Files::new(),
        };
        let mut files = old_files.clone();

        // 1. Which pages may have changed?
        let mut touched: HashSet<PageId> = HashSet::new();
        let mut everything = full;
        if !full {
            let mut after = old_state.last_seq;
            loop {
                let ops = graph.ops_since(after, OPS_BATCH)?;
                let Some(last) = ops.last() else { break };
                after = last.seq;
                for rec in ops {
                    match rec.op {
                        Op::UpsertPage { page } => {
                            // Members list their hubs by title: a renamed hub, or one that
                            // became a note, changes their files.
                            touched.extend(graph.hub_members(page.id)?);
                            touched.insert(page.id);
                        }
                        Op::UpsertBlock { block } => {
                            touched.insert(block.page_id);
                        }
                        // A deleted hub drops out of its members' lists (their `page_hubs`
                        // rows outlive it when no member upsert came along).
                        Op::DeletePage { id } => touched.extend(graph.hub_members(id)?),
                        // Deletions and moves do not say which page lost a block; those
                        // pages are caught by the `updated_at` scan below.
                        Op::DeleteBlock { .. } | Op::PutAsset { .. } | Op::PutTypeDef { .. } => {}
                        // The previous owner of an alias is unknown.
                        Op::SetAlias { .. } => everything = true,
                    }
                }
            }
        }
        let mut current: HashMap<String, Page> = HashMap::new();
        let mut offset = 0;
        loop {
            let batch = graph.list_pages(true, 500, offset)?;
            if batch.is_empty() {
                break;
            }
            offset += batch.len() as u32;
            for p in batch {
                if p.updated_at >= old_state.snapshot_ms {
                    touched.insert(p.id);
                }
                current.insert(export::page_path(&p), p);
            }
        }
        let referenced = export::referenced_blocks(graph.conn())?;
        let old_ref: HashSet<BlockId> = old_state.referenced.iter().copied().collect();
        for id in referenced.symmetric_difference(&old_ref) {
            if let Some(b) = graph.get_block(*id)? {
                touched.insert(b.page_id);
            }
        }

        // 2. Rewrite touched pages, drop vanished ones.
        let (mut created, mut edited, mut deleted) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut paths: Vec<&String> = current.keys().collect();
        paths.sort();
        for path in paths {
            let page = &current[path];
            if !everything && !touched.contains(&page.id) {
                continue;
            }
            let text = export::render_page(graph, page, &referenced)?;
            let oid = self.repo.write_blob(text.as_bytes())?;
            match old_files.get(path) {
                Some(old) if *old == oid => {}
                Some(_) => {
                    edited.insert(page.title.clone());
                }
                None => {
                    created.insert(page.title.clone());
                }
            }
            files.insert(path.clone(), oid);
        }
        for path in old_files.keys() {
            let is_page = path.starts_with(&format!("{PAGES_DIR}/"))
                || path.starts_with(&format!("{JOURNALS_DIR}/"));
            if is_page && !current.contains_key(path) {
                files.remove(path);
                deleted.insert(export::title_from_path(path).unwrap_or_else(|| path.clone()));
            }
        }

        // 3. Assets.
        let manifest = self
            .repo
            .write_blob(export::render_manifest(graph.conn())?.as_bytes())?;
        let assets_changed = old_files.get(MANIFEST) != Some(&manifest);
        files.insert(MANIFEST.to_string(), manifest);
        let types = match export::render_types(graph)? {
            Some(text) => Some(self.repo.write_blob(text.as_bytes())?),
            None => None,
        };
        let types_changed = old_files.get(TYPES_FILE) != types.as_ref();
        match types {
            Some(oid) => files.insert(TYPES_FILE.to_string(), oid),
            None => files.remove(TYPES_FILE),
        };
        let other = if assets_changed {
            Some("assets updated")
        } else if types_changed {
            Some("types updated")
        } else {
            None
        };
        if self.opts.include_blobs {
            self.add_blobs(graph, &mut files)?;
        }

        // 4. Commit.
        let new_state = State {
            last_seq: seq_now,
            snapshot_ms: graph.now_ms(),
            referenced: referenced.into_iter().collect(),
        };
        if files == old_files && head.is_some() {
            self.save_state(&new_state)?;
            return Ok(None);
        }
        let message = commit_message(&created, &edited, &deleted, other, seq_now);
        let id = self.repo.commit(&files, &message, now_ms / 1000)?;
        self.save_state(&new_state)?;
        Ok(Some(self.repo.commit_info(id)?.into()))
    }

    fn add_blobs(&self, graph: &Graph, files: &mut Files) -> Result<()> {
        let mut st = graph
            .conn()
            .prepare("SELECT hash FROM assets ORDER BY hash")?;
        let hashes: Vec<String> = st
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for h in hashes {
            let path = format!("blobs/{h}");
            if files.contains_key(&path) || !graph.blobs().contains(&h) {
                continue;
            }
            let bytes = std::fs::read(graph.blobs().path_of(&h))?;
            files.insert(path, self.repo.write_blob(&bytes)?);
        }
        Ok(())
    }

    // ------------------------------------------------------------- queries

    /// Snapshots, newest first.
    pub fn snapshots(&self, limit: usize) -> Result<Vec<SnapshotInfo>> {
        Ok(self.repo.log(limit)?.into_iter().map(Into::into).collect())
    }

    /// Resolves a snapshot reference: full or abbreviated commit id, or `HEAD`.
    pub fn resolve(&self, rev: &str) -> Result<gix::ObjectId> {
        if rev == "HEAD" {
            return self
                .repo
                .head()?
                .ok_or_else(|| Self::not_found("empty history"));
        }
        if rev.len() >= 4 {
            let hits: Vec<_> = self
                .repo
                .log(usize::MAX)?
                .into_iter()
                .filter(|c| c.id.starts_with(rev))
                .collect();
            if let [one] = hits.as_slice() {
                return Repo::parse_id(&one.id);
            }
        }
        Err(Self::not_found(&format!("snapshot {rev}")))
    }

    /// Path of a page's file for a title (journals are titled `YYYY-MM-DD`).
    pub fn path_for_title(title: &str) -> String {
        match chrono::NaiveDate::parse_from_str(title, "%Y-%m-%d") {
            Ok(d) => format!("{JOURNALS_DIR}/{}.md", d.format("%Y_%m_%d")),
            Err(_) => format!("{PAGES_DIR}/{}.md", export::encode_title(title)),
        }
    }

    /// File content at a snapshot, `None` if the page did not exist then.
    pub fn page_at(&self, rev: &str, title: &str) -> Result<Option<String>> {
        self.file_at(self.resolve(rev)?, &Self::path_for_title(title))
    }

    /// Any file of a snapshot by repository path (e.g. `assets.json`).
    pub fn file(&self, rev: &str, path: &str) -> Result<Option<String>> {
        self.file_at(self.resolve(rev)?, path)
    }

    pub(crate) fn file_at(&self, commit: gix::ObjectId, path: &str) -> Result<Option<String>> {
        match self.repo.files(commit)?.get(path) {
            Some(oid) => Ok(Some(
                String::from_utf8_lossy(&self.repo.read_blob(*oid)?).into_owned(),
            )),
            None => Ok(None),
        }
    }

    /// Snapshots in which the page's file changed (newest first), including its creation and
    /// deletion. `exists` tells whether the page was present after that snapshot.
    pub fn page_history(&self, title: &str) -> Result<Vec<PageVersion>> {
        let path = Self::path_for_title(title);
        let mut commits = Vec::new();
        let mut cur = self.repo.head()?;
        while let Some(id) = cur {
            let files = self.repo.files(id)?;
            commits.push((id, files.get(&path).copied()));
            cur = self.repo.parent(id)?;
        }
        let mut out = Vec::new();
        for (i, (id, oid)) in commits.iter().enumerate() {
            let prev = commits.get(i + 1).and_then(|(_, o)| *o);
            if *oid != prev {
                out.push(PageVersion {
                    snapshot: self.repo.commit_info(*id)?.into(),
                    exists: oid.is_some(),
                });
            }
        }
        Ok(out)
    }

    /// Unified diff of a page between two snapshots (`from` may be `None` for "empty").
    pub fn diff_page(&self, title: &str, from: &str, to: &str) -> Result<String> {
        let a = self.page_at(from, title)?.unwrap_or_default();
        let b = self.page_at(to, title)?.unwrap_or_default();
        let diff = similar::TextDiff::from_lines(&a, &b);
        Ok(diff
            .unified_diff()
            .context_radius(3)
            .header(
                &format!("{title}@{}", short(from)),
                &format!("{title}@{}", short(to)),
            )
            .to_string())
    }

    pub(crate) fn snapshot_files(&self, rev: &str) -> Result<Files> {
        self.repo.files(self.resolve(rev)?)
    }

    pub(crate) fn read_blob(&self, oid: gix::ObjectId) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.repo.read_blob(oid)?).into_owned())
    }

    pub(crate) fn not_found(what: &str) -> Error {
        Error::NotFound(what.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageVersion {
    pub snapshot: SnapshotInfo,
    pub exists: bool,
}

fn short(rev: &str) -> &str {
    &rev[..rev.len().min(8)]
}

fn commit_message(
    created: &BTreeSet<String>,
    edited: &BTreeSet<String>,
    deleted: &BTreeSet<String>,
    other: Option<&str>,
    seq: i64,
) -> String {
    let plural = |n: usize| if n == 1 { "page" } else { "pages" };
    let mut parts = Vec::new();
    if !edited.is_empty() {
        parts.push(format!("{} {} edited", edited.len(), plural(edited.len())));
    }
    if !created.is_empty() {
        parts.push(format!("{} created", created.len()));
    }
    if !deleted.is_empty() {
        parts.push(format!("{} deleted", deleted.len()));
    }
    if parts.is_empty() {
        parts.push(other.unwrap_or("snapshot").to_string());
    }
    let mut titles: Vec<&String> = edited.iter().chain(created).chain(deleted).collect();
    titles.sort();
    titles.dedup();
    let mut head = parts.join(", ");
    if !titles.is_empty() {
        let shown: Vec<&str> = titles
            .iter()
            .take(TITLES_IN_MESSAGE)
            .map(|s| s.as_str())
            .collect();
        head.push_str(": ");
        head.push_str(&shown.join(", "));
        if titles.len() > TITLES_IN_MESSAGE {
            head.push_str(", …");
        }
    }
    format!("{head}\n\nSynapse-Seq: {seq}\n")
}
