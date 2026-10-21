//! Logseq graph importer into a [`synapse_core::Graph`].
//!
//! Pipeline: scan → stream assets into the blob store → parse files in parallel (rayon) →
//! create page rows (so origin journals and timestamps are exact) → write blocks on one thread.
//! Block ids are deterministic (`id::` when present, else UUIDv5 of page + ordinal), which
//! makes re-imports idempotent.

mod config;
mod names;
mod outline;
mod rewrite;

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

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub existing: ExistingPages,
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
    let docs = dedupe(docs, &mut report.warnings);

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

    // Every core write commits on its own; without fsyncs that is cheap. A crash mid-import
    // only needs a re-import (idempotent), so durability per commit is not required.
    let _ = graph.conn().pragma_update(None, "synchronous", "OFF");
    let result = write_all(graph, opts, &docs, &origin, &mut report, &mut progress);
    let _ = graph.conn().pragma_update(None, "synchronous", "NORMAL");
    result?;
    Ok(report)
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
        match ensure_page(graph, d, origin.get(&d.key).copied()) {
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
        match write_doc(graph, d, pid, existed, opts, report) {
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
        b.content = rewrite::rewrite_dates(&b.content, &cfg.journal_title_format);
        b.content = rewrite::rewrite_assets(&b.content, assets, &mut missing);
        for v in b.properties.values_mut() {
            if let Some(s) = v.as_str() {
                let s = rewrite::rewrite_dates(s, &cfg.journal_title_format);
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

/// Finds or creates the page row. Returns `(id, already existed)`.
fn ensure_page(g: &Graph, d: &Doc, origin: Option<NaiveDate>) -> Result<(PageId, bool)> {
    if let Some(date) = d.journal {
        let existed = g.get_page(synapse_core::store::journal_id(date))?.is_some();
        let p = g.journal(date)?;
        return Ok((p.id, existed));
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
    if existed && opts.existing == ExistingPages::Skip && !existing.is_empty() {
        report.unchanged += 1;
        return Ok(());
    }

    // Final ids: Logseq `id::` when free, else deterministic.
    let mut ids: Vec<Uuid> = Vec::with_capacity(d.blocks.len());
    let mut used: HashSet<Uuid> = HashSet::new();
    for (i, b) in d.blocks.iter().enumerate() {
        let fallback = || Uuid::new_v5(&BLOCK_NS, format!("{}\0{i}", d.key).as_bytes());
        let id = match b.explicit_id {
            Some(id) => {
                let taken_elsewhere =
                    !used.contains(&id) && g.get_block(id)?.is_some_and(|x| x.page_id != pid);
                if used.contains(&id) || taken_elsewhere {
                    report.warnings.push(format!(
                        "{}: duplicate block id {id}, generated a new one",
                        d.rel
                    ));
                    fallback()
                } else {
                    id
                }
            }
            None => fallback(),
        };
        used.insert(id);
        ids.push(id);
    }
    let blocks: Vec<Block> = d
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| Block {
            id: ids[i],
            page_id: pid,
            parent_id: b.parent.map(|p| ids[p]),
            position: b.position.clone(),
            content: b.content.clone(),
            properties: b.properties.clone(),
            collapsed: b.collapsed,
            created_at: d.created,
            updated_at: d.updated,
        })
        .collect();

    let page = g.get_page(pid)?;
    let props_differ = page.as_ref().is_some_and(|p| p.properties != d.props);
    let old: HashMap<Uuid, &Block> = existing.iter().map(|(_, b)| (b.id, b)).collect();
    let same_blocks = old.len() == blocks.len()
        && blocks.iter().all(|b| {
            old.get(&b.id).is_some_and(|o| {
                (
                    o.parent_id,
                    &o.position,
                    &o.content,
                    &o.properties,
                    o.collapsed,
                ) == (
                    b.parent_id,
                    &b.position,
                    &b.content,
                    &b.properties,
                    b.collapsed,
                )
            })
        });
    if existed && same_blocks && !props_differ {
        report.unchanged += 1;
    } else {
        for b in &blocks {
            g.put_block(b)?;
        }
        let stale: Vec<Uuid> = old
            .keys()
            .filter(|id| !used.contains(id))
            .copied()
            .collect();
        for id in stale {
            match g.delete_block(id) {
                Ok(()) | Err(synapse_core::Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        if props_differ {
            g.set_page_properties(pid, d.props.clone())?;
        }
        report.blocks += blocks.len();
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
