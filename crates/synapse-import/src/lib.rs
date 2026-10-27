//! Logseq graph importer into a [`synapse_core::Graph`].
//!
//! Pipeline: scan → stream assets into the blob store → parse files in parallel (rayon) →
//! create page rows (so origin journals and timestamps are exact) → write blocks on one thread.
//! Block ids are deterministic (`id::` when present, else UUIDv5 of page + ordinal), which
//! makes re-imports idempotent.

mod config;
mod names;
pub mod outline;
mod rewrite;
mod track;

use chrono::{Local, NaiveDate, TimeZone};
use config::Config;
use outline::{PBlock, parse_markdown, parse_org_raw, split_aliases};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use synapse_core::parse::extract_refs;
use synapse_core::{Block, Graph, NewPage, PageId, Properties, Result, title_key};
use uuid::Uuid;

pub use config::DateFormat;

/// Namespace for deterministic block ids of blocks without a Logseq `id::`.
/// Namespace of fallback ids (kept for id stability across versions).
const BLOCK_NS: Uuid = Uuid::from_u128(0x2b6d_41c9_0e87_4f3a_9d15_c7a8_e340_5b12);
const CHUNK: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExistingPages {
    /// Rewrite pages whose content differs (blocks are upserted by id, stale ones deleted).
    #[default]
    Update,
    /// Leave pages that already have blocks untouched.
    Skip,
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub existing: ExistingPages,
    /// Skip the files of Logseq's organisational pages (`contents`, `card`, `favorites`,
    /// `hls__*`; see [`is_skipped_legacy`]) and drop the empty stubs their refs would create
    /// in this import. Other built-in names (`TODO`, `NOW`, ...) are imported normally.
    pub skip_legacy_pages: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            existing: ExistingPages::default(),
            skip_legacy_pages: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Scanning,
    Assets,
    Parsing,
    Pages,
    Blocks,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Regular pages created or updated.
    pub pages: usize,
    /// Journal pages created or updated.
    pub journals: usize,
    /// Blocks written.
    pub blocks: usize,
    /// Asset files streamed into the blob store.
    pub assets: usize,
    /// Total size of those asset files.
    pub bytes: u64,
    /// Pages/journals that already existed and were rewritten.
    pub updated: usize,
    /// Pages/journals left alone (identical content, or `ExistingPages::Skip`).
    pub unchanged: usize,
    /// `.org` files imported as raw blocks (relative paths).
    pub org_files: Vec<String>,
    /// Legacy organisational pages left out (relative paths), see `skip_legacy_pages`.
    pub skipped_legacy: Vec<String>,
    pub warnings: Vec<String>,
}

struct FileEntry {
    path: PathBuf,
    rel: String,
    stem: String,
    org: bool,
    journal: bool,
}

struct Doc {
    rel: String,
    title: String,
    key: String,
    journal: Option<NaiveDate>,
    org: bool,
    created: i64,
    updated: i64,
    props: Properties,
    aliases: Vec<String>,
    blocks: Vec<PBlock>,
    /// Lower-cased page titles referenced by this doc (journals only; origin computation).
    refs: Vec<String>,
    warnings: Vec<String>,
}

/// Imports the Logseq graph in `src` into `graph`.
pub fn import_logseq(
    src: &Path,
    graph: &Graph,
    opts: &ImportOptions,
    mut progress: impl FnMut(Progress),
) -> Result<Report> {
    if !src.join("pages").is_dir() && !src.join("journals").is_dir() {
        return Err(synapse_core::Error::Invalid(format!(
            "{} is not a Logseq graph (no pages/ or journals/)",
            src.display()
        )));
    }
    let mut report = Report::default();
    let cfg = Config::load(src);

    progress(Progress {
        phase: Phase::Scanning,
        done: 0,
        total: 0,
    });
    let files = scan(src, &mut report.warnings);
    let asset_files = scan_assets(&src.join("assets"));

    // ---- assets: streamed one by one (constant memory, multi-GB safe)
    let mut asset_map: HashMap<String, String> = HashMap::new();
    for (i, (rel, path)) in asset_files.iter().enumerate() {
        progress(Progress {
            phase: Phase::Assets,
            done: i,
            total: asset_files.len(),
        });
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let mime = name.as_deref().and_then(rewrite::guess_mime);
        match graph.put_asset_file(path, mime) {
            Ok(a) => {
                report.assets += 1;
                report.bytes += a.size;
                asset_map.insert(rel.clone(), a.hash);
            }
            Err(e) => report.warnings.push(format!("asset {rel}: {e}")),
        }
    }

    // ---- parse in parallel, in chunks so progress can be reported from this thread
    let mut docs: Vec<Doc> = Vec::with_capacity(files.len());
    for chunk in files.chunks(CHUNK) {
        progress(Progress {
            phase: Phase::Parsing,
            done: docs.len(),
            total: files.len(),
        });
        let parsed: Vec<Doc> = chunk
            .par_iter()
            .map(|f| parse_doc(f, &cfg, &asset_map))
            .collect();
        docs.extend(parsed);
    }
    let mut docs = dedupe(docs, &mut report.warnings);
    if opts.skip_legacy_pages {
        docs.retain(|d| {
            let legacy = d.journal.is_none() && is_skipped_legacy(&d.title);
            if legacy {
                report.skipped_legacy.push(d.rel.clone());
            }
            !legacy
        });
    }

    // ---- origin journals: earliest journal referencing each page (aliases resolved)
    let mut alias_to_key: HashMap<String, String> = HashMap::new();
    for d in &docs {
        for a in &d.aliases {
            alias_to_key.insert(title_key(a), d.key.clone());
        }
    }
    let mut origin: HashMap<String, NaiveDate> = HashMap::new();
    for d in &docs {
        let Some(date) = d.journal else { continue };
        for r in &d.refs {
            let key = alias_to_key.get(r).unwrap_or(r);
            origin
                .entry(key.clone())
                .and_modify(|o| *o = (*o).min(date))
                .or_insert(date);
        }
    }

    // One transaction for the whole write phase: much faster, and a failed import leaves the
    // graph untouched.
    track::init(graph)?;
    graph.transaction(|g| {
        let before: HashSet<PageId> = if opts.skip_legacy_pages {
            page_ids(g)?
        } else {
            HashSet::new()
        };
        write_all(g, opts, &docs, &origin, &mut report, &mut progress)?;
        if opts.skip_legacy_pages {
            drop_legacy_stubs(g, &before)?;
        }
        Ok(())
    })?;
    Ok(report)
}

/// Logseq files the importer leaves out when `skip_legacy_pages` is set: `contents`, `card`,
/// `favorites` and PDF highlight pages (`hls__*`).
pub fn is_skipped_legacy(title: &str) -> bool {
    let key = title_key(title);
    key.starts_with("hls__") || ["contents", "card", "favorites"].contains(&key.as_str())
}

fn page_ids(g: &Graph) -> Result<HashSet<PageId>> {
    Ok(g.list_pages(false, u32::MAX, 0)?
        .into_iter()
        .map(|p| p.id)
        .collect())
}

/// Refs to `[[contents]]`, `[[card]]`, ... make core create empty stub pages; remove the ones
/// this import created (pages that existed before are never touched).
fn drop_legacy_stubs(g: &Graph, before: &HashSet<PageId>) -> Result<()> {
    let stubs: Vec<PageId> = g
        .list_pages(false, u32::MAX, 0)?
        .into_iter()
        .filter(|p| !before.contains(&p.id) && is_skipped_legacy(&p.title))
        .map(|p| p.id)
        .collect();
    for id in stubs {
        if g.page_tree(id)?.is_empty() {
            g.delete_page(id)?;
        }
    }
    Ok(())
}

fn write_all(
    graph: &Graph,
    opts: &ImportOptions,
    docs: &[Doc],
    origin: &HashMap<String, NaiveDate>,
    report: &mut Report,
    progress: &mut impl FnMut(Progress),
) -> Result<()> {
    // ---- page rows first (so stubs created by block refs can't pre-empt exact metadata)
    let mut order: Vec<usize> = (0..docs.len()).collect();
    order.sort_by(|&a, &b| {
        (docs[a].journal.is_none(), docs[a].journal, &docs[a].key).cmp(&(
            docs[b].journal.is_none(),
            docs[b].journal,
            &docs[b].key,
        ))
    });
    let mut ids: HashMap<usize, (PageId, bool)> = HashMap::new();
    for (n, &i) in order.iter().enumerate() {
        progress(Progress {
            phase: Phase::Pages,
            done: n,
            total: order.len(),
        });
        let d = &docs[i];
        let o = origin.get(&d.key).copied();
        match graph.transaction(|g| ensure_page(g, d, o)) {
            // Hubs are Synapse-only pages without blocks: a re-import never writes them.
            Ok((pid, true)) if graph.get_page(pid)?.is_some_and(|p| p.is_hub()) => {
                report.warnings.push(format!(
                    "{}: {:?} is a hub in Synapse, skipped",
                    d.rel, d.title
                ));
            }
            Ok(r) => {
                ids.insert(i, r);
            }
            Err(e) => report.warnings.push(format!("{}: {e}", d.rel)),
        }
    }

    // ---- aliases before any block, so `[[alias]]` refs don't spawn stub pages
    for (&i, &(pid, _)) in &ids {
        set_aliases(graph, &docs[i], pid, &mut report.warnings)?;
    }

    // ---- blocks
    for (n, &i) in order.iter().enumerate() {
        progress(Progress {
            phase: Phase::Blocks,
            done: n,
            total: order.len(),
        });
        let Some(&(pid, existed)) = ids.get(&i) else {
            continue;
        };
        let d = &docs[i];
        // A failing document rolls back as a whole and becomes a warning.
        match graph.transaction(|g| write_doc(g, d, pid, existed, opts, report)) {
            Ok(()) => {}
            Err(e) => report.warnings.push(format!("{}: {e}", d.rel)),
        }
    }
    progress(Progress {
        phase: Phase::Blocks,
        done: order.len(),
        total: order.len(),
    });
    Ok(())
}

fn scan(src: &Path, warnings: &mut Vec<String>) -> Vec<FileEntry> {
    let mut out = Vec::new();
    for (dir, journal) in [("pages", false), ("journals", true)] {
        let Ok(rd) = fs::read_dir(src.join(dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !path.is_file() {
                continue;
            }
            let Some((stem, ext)) = name.rsplit_once('.') else {
                continue;
            };
            let org = match ext.to_ascii_lowercase().as_str() {
                "md" | "markdown" => false,
                "org" => true,
                _ => {
                    warnings.push(format!("{dir}/{name}: unsupported file type, skipped"));
                    continue;
                }
            };
            out.push(FileEntry {
                path,
                rel: format!("{dir}/{name}"),
                stem: stem.to_string(),
                org,
                journal,
            });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// All files under `assets/` as `(path relative to assets with '/', absolute path)`.
fn scan_assets(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file()
                && let Ok(rel) = p.strip_prefix(root)
            {
                let rel = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    out
}

fn ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn day_of(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

fn parse_doc(f: &FileEntry, cfg: &Config, assets: &HashMap<String, String>) -> Doc {
    let mut warnings = Vec::new();
    let (mut created, mut updated) = (0, 0);
    if let Ok(m) = fs::metadata(&f.path) {
        updated = m.modified().map(ms).unwrap_or(0);
        created = m.created().map(ms).unwrap_or(updated).min(updated);
    }
    let text = match fs::read(&f.path) {
        Ok(b) => {
            let s = String::from_utf8_lossy(&b).into_owned();
            s.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(s)
        }
        Err(e) => {
            warnings.push(format!("{}: {e}", f.rel));
            String::new()
        }
    };
    let journal = if f.journal {
        let d = names::journal_date(&f.stem, cfg);
        if d.is_none() {
            warnings.push(format!(
                "{}: not a journal file name, imported as a page",
                f.rel
            ));
        }
        d
    } else {
        None
    };
    let (mut props, mut blocks) = if f.org {
        (Properties::new(), parse_org_raw(&text))
    } else {
        let o = parse_markdown(&text);
        (o.page_props, o.blocks)
    };
    let mut title = match journal {
        Some(d) => synapse_core::journal_title(d),
        None => names::title_from_stem(&f.stem, cfg),
    };
    if journal.is_none()
        && let Some(t) = props
            .remove("title")
            .and_then(|v| v.as_str().map(str::to_string))
        && !t.trim().is_empty()
    {
        title = t.trim().to_string();
    }
    let aliases = props
        .get("alias")
        .and_then(|v| v.as_str())
        .map(split_aliases)
        .unwrap_or_default();

    let mut missing = Vec::new();
    for b in &mut blocks {
        b.content = rewrite::rewrite_dates(&b.content, &cfg.journal_title_formats);
        b.content = rewrite::rewrite_assets(&b.content, assets, &mut missing);
        for v in b.properties.values_mut() {
            if let Some(s) = v.as_str() {
                let s = rewrite::rewrite_dates(s, &cfg.journal_title_formats);
                *v = serde_json::Value::String(rewrite::rewrite_assets(&s, assets, &mut missing));
            }
        }
    }
    missing.sort();
    missing.dedup();
    for m in missing {
        warnings.push(format!("{}: missing asset {m}", f.rel));
    }

    let mut refs = Vec::new();
    if journal.is_some() {
        let mut seen = HashSet::new();
        for b in &blocks {
            let prop_text = b
                .properties
                .values()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            for t in [&b.content, &prop_text] {
                for (title, _) in extract_refs(t).pages {
                    let k = title_key(&title);
                    if seen.insert(k.clone()) {
                        refs.push(k);
                    }
                }
            }
        }
    }
    Doc {
        rel: f.rel.clone(),
        key: title_key(&title),
        title,
        journal,
        org: f.org,
        created,
        updated,
        props,
        aliases,
        blocks,
        refs,
        warnings,
    }
}

/// Drops docs whose title (or journal date) is already taken; `.md` wins over `.org`.
fn dedupe(mut docs: Vec<Doc>, warnings: &mut Vec<String>) -> Vec<Doc> {
    docs.sort_by(|a, b| (a.org, &a.rel).cmp(&(b.org, &b.rel)));
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut out = Vec::with_capacity(docs.len());
    for mut d in docs {
        warnings.append(&mut d.warnings);
        if let Some(first) = seen.get(&d.key) {
            warnings.push(format!("{}: same page as {first}, skipped", d.rel));
            continue;
        }
        seen.insert(d.key.clone(), d.rel.clone());
        out.push(d);
    }
    out
}

/// Finds or creates the page row. Returns `(id, already existed)`. A page previously imported
/// from the same file is found by source path, so renames made in Synapse are respected.
fn ensure_page(g: &Graph, d: &Doc, origin: Option<NaiveDate>) -> Result<(PageId, bool)> {
    if let Some(date) = d.journal {
        let existed = g.get_page(synapse_core::store::journal_id(date))?.is_some();
        let p = g.journal(date)?;
        return Ok((p.id, existed));
    }
    if let Some(id) = track::page_of(g, &d.rel)?
        && g.get_page(id)?.is_some()
    {
        return Ok((id, true));
    }
    if let Some(p) = g.page_by_title(&d.title)? {
        if title_key(&p.title) != d.key {
            return Err(synapse_core::Error::Conflict(format!(
                "title {:?} is already an alias of {:?}",
                d.title, p.title
            )));
        }
        return Ok((p.id, true));
    }
    let p = g.create_page(NewPage {
        title: d.title.clone(),
        origin_journal: Some(origin.unwrap_or_else(|| day_of(d.created))),
        properties: d.props.clone(),
        created_at: Some(d.created),
        updated_at: Some(d.updated),
    })?;
    Ok((p.id, false))
}

fn write_doc(
    g: &Graph,
    d: &Doc,
    pid: PageId,
    existed: bool,
    opts: &ImportOptions,
    report: &mut Report,
) -> Result<()> {
    let existing = if existed { g.page_tree(pid)? } else { vec![] };
    let tracked = track::blocks_of(g, pid)?;
    let first_import = tracked.is_empty();
    if existed && opts.existing == ExistingPages::Skip && !existing.is_empty() {
        report.unchanged += 1;
        return Ok(());
    }
    let old: HashMap<Uuid, &Block> = existing.iter().map(|(_, b)| (b.id, b)).collect();

    // Final ids: Logseq `id::` when free; otherwise derived from (source file, parent id,
    // content, duplicate index) so edits elsewhere in the file never shift them.
    let mut ids: Vec<Uuid> = Vec::with_capacity(d.blocks.len());
    let mut used: HashSet<Uuid> = HashSet::new();
    let mut dups: HashMap<(Option<Uuid>, String), usize> = HashMap::new();
    for b in &d.blocks {
        let parent = b.parent.map(|p| ids[p]);
        let fallback = |dups: &mut HashMap<(Option<Uuid>, String), usize>| {
            let h = track::id_hash(&b.content, &b.properties);
            let n = dups.entry((parent, h.clone())).or_default();
            let name = format!("{}\0{parent:?}\0{h}\0{n}", d.rel);
            *n += 1;
            Uuid::new_v5(&BLOCK_NS, name.as_bytes())
        };
        let id = match b.explicit_id {
            Some(id) => {
                let elsewhere = !old.contains_key(&id)
                    && !used.contains(&id)
                    && g.get_block(id)?.is_some_and(|x| x.page_id != pid);
                if used.contains(&id) || (elsewhere && !tracked.contains_key(&id)) {
                    report.warnings.push(format!(
                        "{}: duplicate block id {id}, generated a new one",
                        d.rel
                    ));
                    fallback(&mut dups)
                } else {
                    id
                }
            }
            None => fallback(&mut dups),
        };
        used.insert(id);
        ids.push(id);
    }

    // New root blocks of a page that already has content are appended after it.
    let mut roots_after: Option<String> = None;
    if first_import && !existing.is_empty() {
        roots_after = existing
            .iter()
            .filter(|(_, b)| b.parent_id.is_none())
            .map(|(_, b)| b.position.clone())
            .max();
    }

    let mut written = 0usize;
    let mut kept: Vec<String> = Vec::new();
    for (i, b) in d.blocks.iter().enumerate() {
        let mut position = b.position.clone();
        if b.parent.is_none() && roots_after.is_some() {
            position = synapse_core::position::between(roots_after.as_deref(), None);
            roots_after = Some(position.clone());
        }
        let block = Block {
            id: ids[i],
            page_id: pid,
            parent_id: b.parent.map(|p| ids[p]),
            position,
            content: b.content.clone(),
            properties: b.properties.clone(),
            collapsed: b.collapsed,
            created_at: d.created,
            updated_at: d.updated,
        };
        let new_hash = track::state_hash(&block);
        match (old.get(&block.id), tracked.get(&block.id)) {
            (None, None) => {
                // Only explicit ids can collide with blocks living elsewhere.
                if b.explicit_id.is_some()
                    && let Some(other) = g.get_block(block.id)?
                    && other.page_id != pid
                {
                    continue; // moved away from this page in Synapse
                }
                g.put_block(&block)?;
                track::set_block(g, &block, &d.rel, &new_hash)?;
                written += 1;
            }
            (None, Some(h)) => {
                // Deleted in Synapse: only resurrect when the file changed it since.
                if *h != new_hash && g.get_block(block.id)?.is_none() {
                    kept.push(format!("{}: block deleted in Synapse kept deleted", d.rel));
                }
            }
            (Some(cur), Some(h)) => {
                let cur_hash = track::state_hash(cur);
                if cur_hash == new_hash {
                    if *h != new_hash {
                        track::set_block(g, &block, &d.rel, &new_hash)?;
                    }
                } else if cur_hash == *h {
                    // untouched in Synapse: file wins
                    g.put_block(&block)?;
                    track::set_block(g, &block, &d.rel, &new_hash)?;
                    written += 1;
                } else {
                    if *h != new_hash {
                        kept.push(format!(
                            "{}: block {} edited in both Synapse and the file, kept the Synapse version",
                            d.rel, block.id
                        ));
                    }
                    track::set_block(g, cur, &d.rel, &cur_hash)?;
                }
            }
            (Some(cur), None) => {
                // A Synapse block with the same (explicit) id that was never imported.
                if track::state_hash(cur) != new_hash {
                    kept.push(format!(
                        "{}: block {} already exists with other content, kept it",
                        d.rel, block.id
                    ));
                }
                track::set_block(g, cur, &d.rel, &track::state_hash(cur))?;
            }
        }
    }
    report.warnings.extend(kept);

    // Imported blocks that vanished from the file: delete only those untouched in Synapse
    // and without native descendants.
    let mut deleted = 0usize;
    let stale: Vec<(&Uuid, &String)> = tracked
        .iter()
        .filter(|(id, _)| !used.contains(id))
        .collect();
    if !stale.is_empty() {
        let tree = g.page_tree(pid)?;
        let mut kids: HashMap<Uuid, Vec<&Block>> = HashMap::new();
        let mut byid: HashMap<Uuid, &Block> = HashMap::new();
        for (_, b) in &tree {
            byid.insert(b.id, b);
            if let Some(p) = b.parent_id {
                kids.entry(p).or_default().push(b);
            }
        }
        let stale_ids: HashSet<Uuid> = stale.iter().map(|(id, _)| **id).collect();
        for (id, hash) in &stale {
            let Some(b) = byid.get(id) else {
                track::drop_block(g, **id)?;
                continue;
            };
            let mut subtree = vec![*b];
            let mut i = 0;
            while i < subtree.len() {
                if let Some(k) = kids.get(&subtree[i].id) {
                    subtree.extend(k.iter().copied());
                }
                i += 1;
            }
            let clean = subtree.iter().all(|x| {
                stale_ids.contains(&x.id)
                    && tracked
                        .get(&x.id)
                        .is_some_and(|h| *h == track::state_hash(x))
            });
            if track::state_hash(b) == **hash && clean {
                g.delete_block(b.id)?;
                deleted += 1;
                for x in subtree {
                    track::drop_block(g, x.id)?;
                }
            } else {
                report.warnings.push(format!(
                    "{}: block {} was removed from the file but edited or extended in Synapse, kept",
                    d.rel, b.id
                ));
                track::drop_block(g, b.id)?;
            }
        }
    }

    // Page properties: merge into native pages, follow the file while unedited.
    let page = g
        .get_page(pid)?
        .ok_or_else(|| synapse_core::Error::NotFound(pid.to_string()))?;
    let stored = track::page_props_hash(g, &d.rel)?;
    let cur_hash = track::props_hash(&page.properties);
    let new_hash = track::props_hash(&d.props);
    let mut props_changed = false;
    if (existed || d.journal.is_some()) && cur_hash != new_hash {
        let mut next = page.properties.clone();
        match &stored {
            Some(h) if *h == cur_hash => next = d.props.clone(),
            Some(_) => {
                if Some(&new_hash) != stored.as_ref() {
                    report.warnings.push(format!(
                        "{}: page properties edited in Synapse, kept",
                        d.rel
                    ));
                }
            }
            None => {
                for (k, v) in &d.props {
                    next.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        if next != page.properties {
            let mut np = page.clone();
            np.properties = next;
            if d.journal.is_none() {
                np.updated_at = d.updated;
            }
            g.put_page(&np)?;
            props_changed = true;
        }
    }
    let final_props = g.get_page(pid)?.map(|p| p.properties).unwrap_or_default();
    let final_hash = track::props_hash(&final_props);
    if stored.as_deref() != Some(final_hash.as_str()) {
        track::set_page(g, &d.rel, pid, &final_hash)?;
    }

    if existed && written == 0 && deleted == 0 && !props_changed {
        report.unchanged += 1;
    } else {
        report.blocks += written;
        if d.journal.is_some() {
            report.journals += 1;
        } else {
            report.pages += 1;
        }
        if existed {
            report.updated += 1;
        }
        if d.org {
            report.org_files.push(d.rel.clone());
        }
    }
    Ok(())
}

fn set_aliases(g: &Graph, d: &Doc, pid: PageId, warnings: &mut Vec<String>) -> Result<()> {
    for a in &d.aliases {
        if title_key(a) != d.key
            && g.page_by_title(a)?.is_none_or(|p| p.id != pid)
            && let Err(e) = g.set_alias(a, Some(pid))
        {
            warnings.push(format!("{}: alias {a:?}: {e}", d.rel));
        }
    }
    Ok(())
}
