//! Tool implementations: plain functions over a [`Graph`] returning compact Markdown.
//!
//! They only use the public `synapse_core` API (plus read-only queries on the core `pages`,
//! `blocks` and `aliases` tables for full-text scans). Journals are read-only: every write
//! refuses journal pages.

use crate::params::*;
use crate::text::{contains_word, count, fold, one_line, query_terms, snippet, truncate};
use chrono::{Duration, Local, NaiveDate, TimeZone};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use synapse_core::parse::extract_refs;
use synapse_core::{Block, BlockId, Graph, Page, PageId, Placement, journal_title, title_key};
use uuid::Uuid;

/// Error shown to the model as the tool result (never a protocol error).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolError {
    pub message: String,
    /// SQLite was busy (another process holds the write lock); the call may be retried.
    pub busy: bool,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            busy: false,
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<synapse_core::Error> for ToolError {
    fn from(e: synapse_core::Error) -> Self {
        use synapse_core::Error as E;
        match e {
            E::NotFound(m) => Self::new(format!("not found: {m}")),
            E::Invalid(m) => Self::new(m),
            E::Conflict(m) => Self::new(m),
            E::Sqlite(e) => Self::from(e),
            other => Self::new(format!("storage error: {other}")),
        }
    }
}

impl From<rusqlite::Error> for ToolError {
    fn from(e: rusqlite::Error) -> Self {
        let busy = matches!(
            e.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
        );
        Self {
            message: if busy {
                "the graph is busy (another process is writing); try again".into()
            } else {
                format!("storage error: {e}")
            },
            busy,
        }
    }
}

pub type ToolResult = Result<String, ToolError>;

const MAX_RANGE_DAYS: i64 = 31;
const MAX_TIMELINE_DAYS: i64 = 92;

// ------------------------------------------------------------------ helpers

fn err(m: impl Into<String>) -> ToolError {
    ToolError::new(m)
}

fn day_of_ms(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .earliest()
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

fn fmt_day(ms: i64) -> String {
    journal_title(day_of_ms(ms))
}

/// `YYYY-MM-DD`, `today`, `yesterday` or `tomorrow`.
pub fn parse_date(g: &Graph, s: &str) -> Result<NaiveDate, ToolError> {
    let s = s.trim().trim_start_matches("[[").trim_end_matches("]]");
    let today = g.today();
    match s.to_ascii_lowercase().as_str() {
        "today" => Ok(today),
        "yesterday" => Ok(today - Duration::days(1)),
        "tomorrow" => Ok(today + Duration::days(1)),
        _ => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| err(format!("bad date {s:?}: use YYYY-MM-DD or \"today\""))),
    }
}

fn opt_date(g: &Graph, s: &Option<String>) -> Result<Option<NaiveDate>, ToolError> {
    s.as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse_date(g, s))
        .transpose()
}

/// `[from, to]` with defaults (`to` = today, `from` = `to` - `default_days`), capped.
fn date_range(
    g: &Graph,
    from: &Option<String>,
    to: &Option<String>,
    default_days: i64,
    max_days: i64,
) -> Result<(NaiveDate, NaiveDate), ToolError> {
    let to = opt_date(g, to)?.unwrap_or_else(|| g.today());
    let from = opt_date(g, from)?.unwrap_or(to - Duration::days(default_days));
    if from > to {
        return Err(err("`from` is after `to`"));
    }
    if (to - from).num_days() >= max_days {
        return Err(err(format!("range too long: at most {max_days} days")));
    }
    Ok((from, to))
}

fn parse_cursor(c: &Option<String>) -> Result<usize, ToolError> {
    match c.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        None => Ok(0),
        Some(c) => c.parse().map_err(|_| err(format!("bad cursor {c:?}"))),
    }
}

fn limit(l: Option<u32>, default: u32, max: u32) -> usize {
    l.unwrap_or(default).clamp(1, max) as usize
}

fn parse_block_id(s: &str) -> Result<BlockId, ToolError> {
    let s = s
        .trim()
        .trim_start_matches("((")
        .trim_end_matches("))")
        .trim();
    Uuid::parse_str(s).map_err(|_| err(format!("bad block id {s:?}")))
}

fn get_block(g: &Graph, s: &str) -> Result<Block, ToolError> {
    let id = parse_block_id(s)?;
    g.get_block(id)?
        .ok_or_else(|| err(format!("block {id} not found")))
}

/// A page by title, alias, id, `[[Title]]` or journal date.
pub fn resolve_page(g: &Graph, s: &str) -> Result<Page, ToolError> {
    let s = s.trim();
    let s = s
        .strip_prefix("[[")
        .and_then(|x| x.strip_suffix("]]"))
        .unwrap_or(s)
        .trim();
    if s.is_empty() {
        return Err(err("empty page name"));
    }
    if let Ok(id) = Uuid::parse_str(s)
        && let Some(p) = g.get_page(id)?
    {
        return Ok(p);
    }
    if let Some(p) = g.page_by_title(s)? {
        return Ok(p);
    }
    if let Ok(d) = parse_date(g, s) {
        return g
            .get_page(synapse_core::store::journal_id(d))?
            .ok_or_else(|| err(format!("no journal for {}", journal_title(d))));
    }
    let q = fold(s);
    let mut close: Vec<String> = g
        .all_titles()?
        .into_iter()
        .map(|(t, _)| t)
        .filter(|t| fold(t).contains(&q))
        .collect();
    close.sort_by_key(|t| t.chars().count());
    close.dedup();
    close.truncate(5);
    Err(err(if close.is_empty() {
        format!("page {s:?} not found")
    } else {
        format!(
            "page {s:?} not found; did you mean: {}?",
            close
                .iter()
                .map(|t| format!("[[{t}]]"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }))
}

fn resolve_hub(g: &Graph, s: &str) -> Result<Page, ToolError> {
    let p = resolve_page(g, s)?;
    if !p.is_hub() {
        return Err(err(format!(
            "[[{}]] is not a hub (see the `hubs` tool)",
            p.title
        )));
    }
    Ok(p)
}

/// A page the write tools may change: never a journal (journals are read-only over MCP).
fn writable_page(g: &Graph, s: &str) -> Result<Page, ToolError> {
    let p = resolve_page(g, s)?;
    ensure_writable(&p)?;
    Ok(p)
}

fn ensure_writable(p: &Page) -> Result<(), ToolError> {
    if p.is_journal() {
        return Err(err(format!(
            "journal {} is read-only here; write to a page instead",
            p.title
        )));
    }
    if p.is_template() {
        return Err(err("hub templates cannot be edited here"));
    }
    Ok(())
}

fn hub_title(g: &Graph, p: &Page) -> Result<Option<String>, ToolError> {
    match p.hubs.first() {
        Some(h) => Ok(g.get_page(*h)?.filter(Page::is_hub).map(|h| h.title)),
        None => Ok(None),
    }
}

/// `type: X · tags: a, b · hub: H` (only the parts present).
fn page_facets(g: &Graph, p: &Page) -> Result<Vec<String>, ToolError> {
    let mut parts = Vec::new();
    if p.is_hub() {
        parts.push("hub page".to_string());
    }
    if let Some(t) = p.page_type() {
        parts.push(format!("type: {t}"));
    }
    if !p.tags.is_empty() {
        parts.push(format!("tags: {}", p.tags.join(", ")));
    }
    if let Some(h) = hub_title(g, p)? {
        parts.push(format!("hub: {h}"));
    }
    Ok(parts)
}

fn props_inline(props: &synapse_core::Properties) -> String {
    props
        .iter()
        .map(|(k, v)| match v.as_str() {
            Some(s) => format!("{k}: {}", truncate(s, 120)),
            None => format!("{k}: {}", truncate(&v.to_string(), 120)),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One outline block as indented Markdown (continuation lines and `key:: value` properties
/// indented under the bullet).
fn render_block(out: &mut String, depth: usize, b: &Block, ids: bool) {
    let pad = "  ".repeat(depth);
    let mut lines = b.content.lines();
    let first = lines.next().unwrap_or("");
    let _ = write!(out, "{pad}- {first}");
    if ids {
        let _ = write!(out, " (({}))", b.id);
    }
    out.push('\n');
    for l in lines {
        let _ = writeln!(out, "{pad}  {l}");
    }
    for (k, v) in &b.properties {
        let v = v
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string());
        let _ = writeln!(out, "{pad}  {k}:: {v}");
    }
}

fn page_block_count(g: &Graph, page: PageId) -> Result<i64, ToolError> {
    Ok(g.conn().query_row(
        "SELECT count(*) FROM blocks WHERE page_id=?1",
        [page.to_string()],
        |r| r.get(0),
    )?)
}

// -------------------------------------------------------------------- reads

/// `get_page`: header (type, tags, hub, aliases, properties, dates) + outline.
pub fn get_page(g: &Graph, p: &GetPageParams) -> ToolResult {
    let page = resolve_page(g, &p.page)?;
    let ids = p.ids.unwrap_or(true);
    let mut out = format!("# {}\n", page.title);
    let _ = writeln!(out, "- id: {}", page.id);
    for f in page_facets(g, &page)? {
        let _ = writeln!(out, "- {f}");
    }
    let aliases = g.aliases_of(page.id)?;
    if !aliases.is_empty() {
        let _ = writeln!(out, "- aliases: {}", aliases.join(", "));
    }
    if !page.properties.is_empty() {
        let _ = writeln!(out, "- properties: {}", props_inline(&page.properties));
    }
    if !page.is_journal() {
        let mut dates = format!(
            "- created: {} · updated: {}",
            fmt_day(page.created_at),
            fmt_day(page.updated_at)
        );
        if let Some(o) = page.origin_journal {
            let _ = write!(dates, " · born in [[{}]]", journal_title(o));
        }
        let _ = writeln!(out, "{dates}");
    }
    if page.is_hub() {
        if !page.description.is_empty() {
            let _ = writeln!(out, "\n{}", page.description);
        }
        let members = g.hub_members(page.id)?;
        let _ = writeln!(out, "\n## Members ({})", members.len());
        for m in members.iter().take(300) {
            if let Some(mp) = g.get_page(*m)? {
                let mark = if mp.is_hub() { " (hub)" } else { "" };
                let _ = writeln!(out, "- [[{}]]{mark}", mp.title);
            }
        }
        if members.len() > 300 {
            let _ = writeln!(out, "- … {} more", members.len() - 300);
        }
        return Ok(out);
    }
    let mut tree = g.page_tree(page.id)?;
    if let Some(b) = &p.block {
        let id = parse_block_id(b)?;
        let start = tree
            .iter()
            .position(|(_, b)| b.id == id)
            .ok_or_else(|| err(format!("block {id} is not on [[{}]]", page.title)))?;
        let base = tree[start].0;
        let end = tree[start + 1..]
            .iter()
            .position(|(d, _)| *d <= base)
            .map_or(tree.len(), |i| start + 1 + i);
        tree = tree.drain(start..end).map(|(d, b)| (d - base, b)).collect();
    }
    if let Some(max) = p.max_depth {
        tree.retain(|(d, _)| *d <= max as usize);
    }
    let total = tree.len();
    let offset = parse_cursor(&p.cursor)?;
    let lim = limit(p.limit, 400, 2000);
    out.push('\n');
    if total == 0 {
        out.push_str("(no blocks)\n");
    }
    for (d, b) in tree.iter().skip(offset).take(lim) {
        render_block(&mut out, *d, b, ids);
    }
    if offset + lim < total {
        let _ = writeln!(
            out,
            "\n… {} more blocks; next cursor: {}",
            total - offset - lim,
            offset + lim
        );
    }
    Ok(out)
}

/// `get_journal`: one day, or every existing journal in a range (oldest first).
pub fn get_journal(g: &Graph, p: &GetJournalParams) -> ToolResult {
    let ids = p.ids.unwrap_or(true);
    let days: Vec<NaiveDate> = if p.from.is_some() || p.to.is_some() {
        let (from, to) = date_range(g, &p.from, &p.to, 6, MAX_RANGE_DAYS)?;
        let mut v: Vec<NaiveDate> = g
            .journals_between(from, to)?
            .into_iter()
            .filter_map(|j| j.journal_date)
            .collect();
        v.reverse();
        if v.is_empty() {
            return Ok(format!(
                "No journals between {} and {}.",
                journal_title(from),
                journal_title(to)
            ));
        }
        v
    } else {
        vec![match &p.date {
            Some(d) if !d.trim().is_empty() => parse_date(g, d)?,
            _ => g.today(),
        }]
    };
    let mut out = String::new();
    let mut budget = 1500usize;
    for d in days {
        let title = journal_title(d);
        // Never `Graph::journal`: it would create the page.
        let Some(page) = g.get_page(synapse_core::store::journal_id(d))? else {
            let _ = writeln!(out, "# {title}\n(no journal)\n");
            continue;
        };
        let tree = g.page_tree(page.id)?;
        let _ = writeln!(out, "# {title}");
        if tree.is_empty() {
            out.push_str("(empty)\n");
        }
        for (depth, b) in &tree {
            if budget == 0 {
                out.push_str("… (output truncated; ask for a shorter range)\n");
                return Ok(out);
            }
            budget -= 1;
            render_block(&mut out, *depth, b, ids);
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_string() + "\n")
}

struct PageRow {
    title: String,
    journal: Option<NaiveDate>,
    updated_at: i64,
    template: bool,
    hub: bool,
}

fn page_rows(g: &Graph) -> Result<HashMap<PageId, PageRow>, ToolError> {
    let mut st = g
        .conn()
        .prepare("SELECT id, title, journal_date, updated_at, kind FROM pages")?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = HashMap::new();
    for r in rows {
        let (id, title, jd, updated_at, kind) = r?;
        let Ok(id) = Uuid::parse_str(&id) else {
            continue;
        };
        out.insert(
            id,
            PageRow {
                title,
                journal: jd.and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()),
                updated_at,
                template: kind == "template",
                hub: kind == "hub",
            },
        );
    }
    Ok(out)
}

/// Pages allowed by the tag/type/hub/page filters (`None` = no filter).
fn page_filter(
    g: &Graph,
    tag: &Option<String>,
    ty: &Option<String>,
    hub: &Option<String>,
    page: &Option<String>,
) -> Result<Option<HashSet<PageId>>, ToolError> {
    let mut set: Option<HashSet<PageId>> = None;
    let mut narrow = |s: HashSet<PageId>| {
        set = Some(match set.take() {
            Some(cur) => cur.intersection(&s).copied().collect(),
            None => s,
        });
    };
    if let Some(t) = tag.as_deref().filter(|t| !t.trim().is_empty()) {
        narrow(g.pages_with_tag(t)?.into_iter().collect());
    }
    if let Some(t) = ty.as_deref().filter(|t| !t.trim().is_empty()) {
        let key = title_key(t);
        narrow(
            g.all_page_types()?
                .into_iter()
                .filter(|(_, ty)| title_key(ty) == key)
                .map(|(id, _)| id)
                .collect(),
        );
    }
    if let Some(h) = hub.as_deref().filter(|h| !h.trim().is_empty()) {
        let hub = resolve_hub(g, h)?;
        narrow(g.hub_members(hub.id)?.into_iter().collect());
    }
    if let Some(p) = page.as_deref().filter(|p| !p.trim().is_empty()) {
        narrow(std::iter::once(resolve_page(g, p)?.id).collect());
    }
    Ok(set)
}

/// `search`: substring scan over page titles/aliases and block text (all terms must match,
/// case/accent-insensitive), ranked by term frequency, title hits first.
pub fn search(g: &Graph, p: &SearchParams) -> ToolResult {
    let terms = query_terms(&p.query);
    if terms.is_empty() {
        return Err(err("empty query"));
    }
    let scope = p.scope.as_deref().unwrap_or("all");
    let (want_pages, want_blocks) = match scope {
        "all" | "" => (true, true),
        "pages" => (true, false),
        "blocks" => (false, true),
        s => return Err(err(format!("bad scope {s:?}: all, pages or blocks"))),
    };
    let journals = p.journals.as_deref().unwrap_or("any");
    if !matches!(journals, "any" | "only" | "exclude" | "") {
        return Err(err(format!(
            "bad journals {journals:?}: any, only or exclude"
        )));
    }
    let from = opt_date(g, &p.from)?;
    let to = opt_date(g, &p.to)?;
    let allowed = page_filter(g, &p.tag, &p.page_type, &p.hub, &p.page)?;
    let pages = page_rows(g)?;
    let keep = |id: &PageId, day: NaiveDate| -> bool {
        let Some(row) = pages.get(id) else {
            return false;
        };
        if row.template || allowed.as_ref().is_some_and(|a| !a.contains(id)) {
            return false;
        }
        match journals {
            "only" if row.journal.is_none() => return false,
            "exclude" if row.journal.is_some() => return false,
            _ => {}
        }
        !(from.is_some_and(|f| day < f) || to.is_some_and(|t| day > t))
    };
    let phrase = fold(&one_line(&p.query.replace('"', "")));
    // (score, updated_at, line)
    let mut hits: Vec<(f32, i64, String)> = Vec::new();
    if want_pages {
        let mut aliases: HashMap<PageId, Vec<String>> = HashMap::new();
        let mut st = g.conn().prepare("SELECT page_id, alias FROM aliases")?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, a) = r?;
            if let Ok(id) = Uuid::parse_str(&id) {
                aliases.entry(id).or_default().push(a);
            }
        }
        for (id, row) in &pages {
            let day = row.journal.unwrap_or_else(|| day_of_ms(row.updated_at));
            if !keep(id, day) {
                continue;
            }
            let names = aliases.get(id).map(|a| a.join(" ")).unwrap_or_default();
            let hay = fold(&format!("{} {names}", row.title));
            if !terms.iter().all(|t| hay.contains(t.as_str())) {
                continue;
            }
            let exact = fold(&row.title) == phrase;
            let score = 100.0 + if exact { 100.0 } else { 0.0 };
            let mut line = format!("- [[{}]] (page", row.title);
            if row.hub {
                line.push_str(", hub");
            }
            if !names.is_empty() {
                let _ = write!(line, "; aliases: {}", aliases[id].join(", "));
            }
            line.push(')');
            hits.push((score, row.updated_at, line));
        }
    }
    if want_blocks {
        let mut st = g
            .conn()
            .prepare("SELECT id, page_id, content, updated_at FROM blocks")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for r in rows {
            let (id, page_id, content, updated_at) = r?;
            let Ok(pid) = Uuid::parse_str(&page_id) else {
                continue;
            };
            let Some(row) = pages.get(&pid) else {
                continue;
            };
            let day = row.journal.unwrap_or_else(|| day_of_ms(updated_at));
            if !keep(&pid, day) {
                continue;
            }
            let hay = fold(&content);
            if !terms.iter().all(|t| hay.contains(t.as_str())) {
                continue;
            }
            let freq: usize = terms.iter().map(|t| count(&hay, t).min(5)).sum();
            let len = hay.chars().count() as f32;
            let mut score = freq as f32 / (1.0 + len / 400.0);
            if terms.len() > 1 && hay.contains(&phrase) {
                score += 2.0;
            }
            let line = format!(
                "- [[{}]] (({id})) {}: {}",
                row.title,
                journal_title(day),
                snippet(&content, &terms, 200)
            );
            hits.push((score, updated_at, line));
        }
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
    let total = hits.len();
    if total == 0 {
        return Ok(format!("No results for {:?}.", p.query));
    }
    let offset = parse_cursor(&p.cursor)?;
    let lim = limit(p.limit, 20, 100);
    let shown: Vec<&str> = hits
        .iter()
        .skip(offset)
        .take(lim)
        .map(|h| h.2.as_str())
        .collect();
    let mut out = format!(
        "{total} results for {:?} (showing {}-{}):\n",
        p.query,
        offset + 1,
        offset + shown.len()
    );
    for l in shown {
        out.push_str(l);
        out.push('\n');
    }
    if offset + lim < total {
        let _ = writeln!(out, "next cursor: {}", offset + lim);
    }
    Ok(out)
}

/// `list_pages`: filtered, sorted, paginated page index.
pub fn list_pages(g: &Graph, p: &ListPagesParams) -> ToolResult {
    let journals = p.journals.as_deref().unwrap_or("exclude");
    let include = match journals {
        "exclude" | "" => false,
        "include" | "only" => true,
        j => return Err(err(format!("bad journals {j:?}: exclude, include or only"))),
    };
    let updated_since = opt_date(g, &p.updated_since)?;
    let created_since = opt_date(g, &p.created_since)?;
    let allowed = page_filter(g, &p.tag, &p.page_type, &p.hub, &None)?;
    let untyped = p.untyped.unwrap_or(false);
    let untagged = p.untagged.unwrap_or(false);
    let mut pages: Vec<Page> = g
        .list_pages(include, u32::MAX, 0)?
        .into_iter()
        .filter(|pg| journals != "only" || pg.is_journal())
        .filter(|pg| allowed.as_ref().is_none_or(|a| a.contains(&pg.id)))
        .filter(|pg| !untyped || (!pg.is_journal() && !pg.is_hub() && pg.types.is_empty()))
        .filter(|pg| !untagged || (!pg.is_journal() && !pg.is_hub() && pg.tags.is_empty()))
        .filter(|pg| updated_since.is_none_or(|d| day_of_ms(pg.updated_at) >= d))
        .filter(|pg| created_since.is_none_or(|d| day_of_ms(pg.created_at) >= d))
        .collect();
    match p.sort.as_deref().unwrap_or("updated") {
        "updated" | "" => pages.sort_by_key(|p| std::cmp::Reverse(p.updated_at)),
        "created" => pages.sort_by_key(|p| std::cmp::Reverse(p.created_at)),
        "title" => pages.sort_by_cached_key(|p| title_key(&p.title)),
        s => return Err(err(format!("bad sort {s:?}: updated, created or title"))),
    }
    let total = pages.len();
    if total == 0 {
        return Ok("No pages match.".into());
    }
    let offset = parse_cursor(&p.cursor)?;
    let lim = limit(p.limit, 50, 200);
    let mut out = format!(
        "{total} pages (showing {}-{}):\n",
        offset + 1,
        (offset + lim).min(total)
    );
    for pg in pages.iter().skip(offset).take(lim) {
        let mut parts = page_facets(g, pg)?;
        if !pg.is_journal() {
            parts.push(format!("updated {}", fmt_day(pg.updated_at)));
        }
        if parts.is_empty() {
            let _ = writeln!(out, "- [[{}]]", pg.title);
        } else {
            let _ = writeln!(out, "- [[{}]] · {}", pg.title, parts.join(" · "));
        }
    }
    if offset + lim < total {
        let _ = writeln!(out, "next cursor: {}", offset + lim);
    }
    Ok(out)
}

/// `backlinks`: blocks linking to the page, grouped by source page, with parent context.
pub fn backlinks(g: &Graph, p: &PageRefParams) -> ToolResult {
    let page = resolve_page(g, &p.page)?;
    let links = g.backlinks(page.id)?;
    if links.is_empty() {
        return Ok(format!("No linked references to [[{}]].", page.title));
    }
    let total = links.len();
    let offset = parse_cursor(&p.cursor)?;
    let lim = limit(p.limit, 50, 200);
    let n_pages = links
        .iter()
        .map(|l| l.block.page_id)
        .collect::<HashSet<_>>()
        .len();
    let mut out = format!(
        "{total} linked references to [[{}]] from {n_pages} pages (showing {}-{}):\n",
        page.title,
        offset + 1,
        (offset + lim).min(total)
    );
    let mut current: Option<PageId> = None;
    for l in links.iter().skip(offset).take(lim) {
        if current != Some(l.block.page_id) {
            current = Some(l.block.page_id);
            let _ = writeln!(out, "\n## [[{}]]", l.page_title);
        }
        if let Some(parent) = l
            .block
            .parent_id
            .and_then(|id| g.get_block(id).ok().flatten())
        {
            let _ = writeln!(out, "- under: {}", truncate(&parent.content, 100));
        }
        let _ = writeln!(
            out,
            "- {} (({}))",
            truncate(&l.block.content, 400),
            l.block.id
        );
    }
    if offset + lim < total {
        let _ = writeln!(out, "\nnext cursor: {}", offset + lim);
    }
    Ok(out)
}

/// `related`: spreading activation over links, synapses and time.
pub fn related(g: &Graph, p: &PageRefParams) -> ToolResult {
    let page = resolve_page(g, &p.page)?;
    let lim = limit(p.limit, 15, 100);
    let rel = g.related(page.id, lim)?;
    if rel.is_empty() {
        return Ok(format!("Nothing related to [[{}]] yet.", page.title));
    }
    let mut out = format!("Related to [[{}]] (strongest first):\n", page.title);
    for (id, score) in rel {
        if let Some(r) = g.get_page(id)? {
            let _ = writeln!(out, "- [[{}]] ({score:.2})", r.title);
        }
    }
    Ok(out)
}

/// `list_tags`: tags in use with page counts.
pub fn list_tags(g: &Graph, p: &ListParams) -> ToolResult {
    let tags = g.tag_catalog()?;
    if tags.is_empty() {
        return Ok("No tags in use.".into());
    }
    let lim = limit(p.limit, 100, 1000);
    let mut out = format!(
        "{} tags (pages: total; explicit/from type/from hub):\n",
        tags.len()
    );
    for t in tags.iter().take(lim) {
        let _ = writeln!(
            out,
            "- {} — {} ({}/{}/{})",
            t.name, t.count, t.explicit, t.typed, t.from_hub
        );
    }
    if tags.len() > lim {
        let _ = writeln!(out, "… {} more (raise `limit`)", tags.len() - lim);
    }
    Ok(out)
}

/// `list_types`: types in use (with counts and icons), defined ones, then suggestions.
pub fn list_types(g: &Graph, p: &ListParams) -> ToolResult {
    let types = g.type_catalog()?;
    let lim = limit(p.limit, 100, 1000);
    let (used, unused): (Vec<_>, Vec<_>) = types.iter().partition(|t| t.count > 0);
    let mut out = format!("{} types in use:\n", used.len());
    for t in used.iter().take(lim) {
        let icon = t
            .icon
            .as_deref()
            .map(|i| format!("{i} "))
            .unwrap_or_default();
        let _ = writeln!(out, "- {icon}{} — {} pages", t.name, t.count);
    }
    let defined: Vec<&str> = unused
        .iter()
        .filter(|t| !t.suggested)
        .map(|t| t.name.as_str())
        .collect();
    let suggested: Vec<&str> = unused
        .iter()
        .filter(|t| t.suggested)
        .map(|t| t.name.as_str())
        .collect();
    if !defined.is_empty() {
        let _ = writeln!(out, "Defined, unused: {}", defined.join(", "));
    }
    if !suggested.is_empty() {
        let _ = writeln!(out, "Suggested, unused: {}", suggested.join(", "));
    }
    Ok(out)
}

/// `hubs`: the hub tree (or one hub's members).
pub fn hubs(g: &Graph, p: &HubsParams) -> ToolResult {
    if let Some(h) = p.hub.as_deref().filter(|h| !h.trim().is_empty()) {
        let hub = resolve_hub(g, h)?;
        let members = g.hub_members(hub.id)?;
        let mut out = format!("# {} ({} members)\n", hub.title, members.len());
        if !hub.description.is_empty() {
            let _ = writeln!(out, "{}", truncate(&hub.description, 300));
        }
        for m in members {
            if let Some(mp) = g.get_page(m)? {
                let mut parts = page_facets(g, &mp)?;
                parts.retain(|f| !f.starts_with("hub:"));
                let suffix = if parts.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", parts.join(" · "))
                };
                let _ = writeln!(out, "- [[{}]]{suffix}", mp.title);
            }
        }
        return Ok(out);
    }
    let st = g.structure()?;
    if st.hubs.is_empty() {
        return Ok("No hubs.".into());
    }
    let by_id: HashMap<PageId, &synapse_core::HubInfo> =
        st.hubs.iter().map(|h| (h.page, h)).collect();
    let nested: HashSet<PageId> = st
        .hubs
        .iter()
        .flat_map(|h| h.members.iter().copied())
        .filter(|m| by_id.contains_key(m))
        .collect();
    let mut roots: Vec<&synapse_core::HubInfo> = st
        .hubs
        .iter()
        .filter(|h| !nested.contains(&h.page))
        .collect();
    roots.sort_by_cached_key(|h| title_key(&h.title));
    let members = p.members.unwrap_or(false);
    let mut out = format!("{} hubs:\n", st.hubs.len());
    let mut seen = HashSet::new();
    fn walk(
        g: &Graph,
        out: &mut String,
        h: &synapse_core::HubInfo,
        depth: usize,
        by_id: &HashMap<PageId, &synapse_core::HubInfo>,
        seen: &mut HashSet<PageId>,
        members: bool,
    ) -> Result<(), ToolError> {
        if !seen.insert(h.page) {
            return Ok(());
        }
        let pad = "  ".repeat(depth);
        let notes: Vec<PageId> = h
            .members
            .iter()
            .copied()
            .filter(|m| !by_id.contains_key(m))
            .collect();
        let desc = if h.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", truncate(&h.description, 80))
        };
        let _ = writeln!(out, "{pad}- [[{}]] ({} notes){desc}", h.title, notes.len());
        let mut subs: Vec<&synapse_core::HubInfo> = h
            .members
            .iter()
            .filter_map(|m| by_id.get(m).copied())
            .collect();
        subs.sort_by_cached_key(|s| title_key(&s.title));
        for s in subs {
            walk(g, out, s, depth + 1, by_id, seen, members)?;
        }
        if members {
            for n in notes {
                if let Some(np) = g.get_page(n)? {
                    let _ = writeln!(out, "{pad}  - {}", np.title);
                }
            }
        }
        Ok(())
    }
    for r in roots {
        walk(g, &mut out, r, 0, &by_id, &mut seen, members)?;
    }
    // Hubs only reachable through a cycle.
    for h in &st.hubs {
        walk(g, &mut out, h, 0, &by_id, &mut seen, members)?;
    }
    Ok(out)
}

/// `timeline`: per day, the journal, pages born and pages edited.
pub fn timeline(g: &Graph, p: &RangeParams) -> ToolResult {
    let (from, to) = date_range(g, &p.from, &p.to, 6, MAX_TIMELINE_DAYS)?;
    let days = g.timeline(from, to)?;
    let mut titles: HashMap<PageId, String> = HashMap::new();
    let mut title = |id: PageId| -> Result<String, ToolError> {
        if let Some(t) = titles.get(&id) {
            return Ok(t.clone());
        }
        let t = g.get_page(id)?.map(|p| p.title).unwrap_or_default();
        titles.insert(id, t.clone());
        Ok(t)
    };
    let mut out = format!(
        "Timeline {} → {}:\n",
        journal_title(from),
        journal_title(to)
    );
    let mut any = false;
    for d in days {
        if d.journal.is_none() && d.created.is_empty() && d.edited.is_empty() {
            continue;
        }
        any = true;
        let mark = if d.journal.is_some() {
            " (journal)"
        } else {
            ""
        };
        let _ = writeln!(out, "\n## {}{mark}", journal_title(d.date));
        if !d.created.is_empty() {
            let mut names = Vec::new();
            for id in &d.created {
                names.push(format!("[[{}]]", title(*id)?));
            }
            let _ = writeln!(out, "- born: {}", names.join(", "));
        }
        if !d.edited.is_empty() {
            let mut names = Vec::new();
            for (id, n) in &d.edited {
                names.push(format!("[[{}]] ×{n}", title(*id)?));
            }
            let _ = writeln!(out, "- edited: {}", names.join(", "));
        }
    }
    if !any {
        out.push_str("(nothing)\n");
    }
    Ok(out)
}

/// `journal_links`: per journal day, the pages it links (with their curation status) and
/// existing pages it mentions without linking.
pub fn journal_links(g: &Graph, p: &RangeParams) -> ToolResult {
    let (from, to) = date_range(g, &p.from, &p.to, 6, MAX_RANGE_DAYS)?;
    let mut journals = g.journals_between(from, to)?;
    journals.reverse();
    if journals.is_empty() {
        return Ok(format!(
            "No journals between {} and {}.",
            journal_title(from),
            journal_title(to)
        ));
    }
    // Candidate titles for unlinked mentions: notes and hubs, at least 3 chars.
    let candidates: Vec<(String, String)> = g
        .all_titles()?
        .into_iter()
        .filter_map(|(t, id)| {
            let f = fold(t.trim());
            (f.chars().count() >= 3 && NaiveDate::parse_from_str(&t, "%Y-%m-%d").is_err())
                .then_some((f, t))
                .filter(|_| {
                    g.get_page(id)
                        .ok()
                        .flatten()
                        .is_some_and(|p| !p.is_journal() && !p.is_template())
                })
        })
        .collect();
    let mut out = String::new();
    for j in journals {
        let tree = g.page_tree(j.id)?;
        let mut linked: Vec<String> = Vec::new();
        let mut linked_keys: HashSet<String> = HashSet::new();
        let mut text = String::new();
        for (_, b) in &tree {
            for (t, _) in extract_refs(&b.content).pages {
                if linked_keys.insert(title_key(&t)) {
                    linked.push(t);
                }
            }
            text.push_str(&strip_links(&b.content));
            text.push('\n');
        }
        let hay = fold(&text);
        let mut mentions: Vec<&str> = candidates
            .iter()
            .filter(|(f, t)| !linked_keys.contains(&title_key(t)) && contains_word(&hay, f))
            .map(|(_, t)| t.as_str())
            .collect();
        mentions.sort_by_key(|t| title_key(t));
        mentions.dedup_by_key(|t| title_key(t));
        if linked.is_empty() && mentions.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {}", j.title);
        for t in &linked {
            let status = match g.page_by_title(t)? {
                Some(pg) if pg.is_journal() => continue,
                Some(pg) => {
                    let mut s = page_facets(g, &pg)?;
                    if pg.types.is_empty() && !pg.is_hub() {
                        s.push("untyped".into());
                    }
                    if pg.tags.is_empty() && !pg.is_hub() {
                        s.push("untagged".into());
                    }
                    if !pg.is_hub() && page_block_count(g, pg.id)? == 0 {
                        s.push("empty".into());
                    }
                    s.join(" · ")
                }
                None => "missing".into(),
            };
            let _ = writeln!(out, "- linked [[{t}]] · {status}");
        }
        for t in mentions {
            let _ = writeln!(out, "- mentioned (not linked) [[{t}]]");
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("No page links or mentions in that range.\n");
    }
    Ok(out)
}

/// Drops `[[links]]`, `#tags` and `((refs))` so only free text is left.
fn strip_links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[[") {
        out.push_str(&rest[..i]);
        match rest[i..].find("]]") {
            Some(j) => rest = &rest[i + j + 2..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace()
        .filter(|w| !w.starts_with('#') && !w.starts_with("(("))
        .collect::<Vec<_>>()
        .join(" ")
}

// ------------------------------------------------------------------- writes

/// Inserts a Markdown outline under `parent` (or at the page's end). Returns the new
/// top-level blocks and the total count. Logseq `id::` properties are ignored (new ids).
fn insert_outline(
    g: &Graph,
    page: PageId,
    parent: Option<BlockId>,
    text: &str,
) -> Result<(Vec<BlockId>, usize, synapse_core::Properties), ToolError> {
    let outline = synapse_import::outline::parse_markdown(text);
    let mut ids: Vec<BlockId> = Vec::with_capacity(outline.blocks.len());
    let mut top = Vec::new();
    for b in &outline.blocks {
        let under = match b.parent {
            Some(i) => Some(*ids.get(i).ok_or_else(|| err("malformed outline"))?),
            None => parent,
        };
        let blk = g.insert_block(page, Placement::LastChildOf(under), &b.content)?;
        if !b.properties.is_empty() {
            g.set_block_properties(blk.id, b.properties.clone())?;
        }
        if b.collapsed {
            g.set_collapsed(blk.id, true)?;
        }
        if b.parent.is_none() {
            top.push(blk.id);
        }
        ids.push(blk.id);
    }
    Ok((top, ids.len(), outline.page_props))
}

fn clean_list(v: &Option<Vec<String>>) -> Vec<String> {
    v.iter()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `create_page`.
pub fn create_page(g: &Graph, p: &CreatePageParams) -> ToolResult {
    let title = p.title.trim();
    if NaiveDate::parse_from_str(title, "%Y-%m-%d").is_ok() {
        return Err(err("journals are read-only here; pick a non-date title"));
    }
    let origin = match opt_date(g, &p.source_journal)? {
        Some(d) => {
            // Must exist: the page is born there and journals are never created here.
            if g.get_page(synapse_core::store::journal_id(d))?.is_none() {
                return Err(err(format!("no journal for {}", journal_title(d))));
            }
            Some(d)
        }
        None => None,
    };
    let hub = p
        .hub
        .as_deref()
        .filter(|h| !h.trim().is_empty())
        .map(|h| resolve_hub(g, h))
        .transpose()?;
    let page = g.create_page(synapse_core::NewPage {
        title: title.to_string(),
        origin_journal: origin,
        ..Default::default()
    })?;
    if let Some(t) = p.page_type.as_deref().filter(|t| !t.trim().is_empty()) {
        g.set_page_type(page.id, Some(t))?;
    }
    let tags = clean_list(&p.tags);
    if !tags.is_empty() {
        g.set_page_tags(page.id, tags)?;
    }
    if let Some(h) = hub {
        g.add_to_hub(page.id, h.id)?;
    }
    let mut n = 0;
    if let Some(c) = p.content.as_deref().filter(|c| !c.trim().is_empty()) {
        let (_, count, props) = insert_outline(g, page.id, None, c)?;
        n = count;
        if !props.is_empty() {
            g.set_page_properties(page.id, props)?;
        }
    }
    let page = g.get_page(page.id)?.unwrap_or(page);
    let facets = page_facets(g, &page)?;
    let mut out = format!(
        "Created [[{}]] (id {}) with {n} blocks",
        page.title, page.id
    );
    if !facets.is_empty() {
        let _ = write!(out, " · {}", facets.join(" · "));
    }
    out.push('\n');
    Ok(out)
}

/// `append_blocks` (pages only).
pub fn append_blocks(g: &Graph, p: &AppendBlocksParams) -> ToolResult {
    let page = writable_page(g, &p.page)?;
    if page.is_hub() {
        return Err(err("hubs have no blocks; append to a member page"));
    }
    let parent = match p.parent.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(b) => {
            let b = get_block(g, b)?;
            if b.page_id != page.id {
                return Err(err(format!("block {} is not on [[{}]]", b.id, page.title)));
            }
            Some(b.id)
        }
        None => None,
    };
    if p.content.trim().is_empty() {
        return Err(err("empty content"));
    }
    let (top, n, props) = insert_outline(g, page.id, parent, &p.content)?;
    let mut out = format!("Appended {n} blocks to [[{}]]", page.title);
    if !props.is_empty() {
        // Leading `key:: value` lines are page properties (Logseq): merge them.
        let keys: Vec<String> = props.keys().cloned().collect();
        let mut merged = page.properties.clone();
        merged.extend(props);
        g.set_page_properties(page.id, merged)?;
        let _ = write!(out, " (page properties set: {})", keys.join(", "));
    }
    out.push_str(":\n");
    for id in top {
        if let Some(b) = g.get_block(id)? {
            let _ = writeln!(out, "- {} (({id}))", truncate(&b.content, 80));
        }
    }
    Ok(out)
}

fn block_page(g: &Graph, b: &Block) -> Result<Page, ToolError> {
    let page = g
        .get_page(b.page_id)?
        .ok_or_else(|| err(format!("page of block {} not found", b.id)))?;
    ensure_writable(&page)?;
    Ok(page)
}

/// `update_block` (pages only).
pub fn update_block(g: &Graph, p: &UpdateBlockParams) -> ToolResult {
    let b = get_block(g, &p.block)?;
    let page = block_page(g, &b)?;
    let b = g.update_block(b.id, &p.content)?;
    Ok(format!(
        "Updated (({})) on [[{}]]: {}\n",
        b.id,
        page.title,
        truncate(&b.content, 120)
    ))
}

/// `delete_block` (pages only; deletes the subtree).
pub fn delete_block(g: &Graph, p: &DeleteBlockParams) -> ToolResult {
    let b = get_block(g, &p.block)?;
    let page = block_page(g, &b)?;
    let tree = g.page_tree(page.id)?;
    let start = tree.iter().position(|(_, x)| x.id == b.id).unwrap_or(0);
    let base = tree.get(start).map_or(0, |t| t.0);
    let n = 1 + tree[start + 1..]
        .iter()
        .take_while(|(d, _)| *d > base)
        .count();
    g.delete_block(b.id)?;
    Ok(format!(
        "Deleted (({})) and {} children from [[{}]]\n",
        b.id,
        n - 1,
        page.title
    ))
}

/// `set_page_type`.
pub fn set_page_type(g: &Graph, p: &SetPageTypeParams) -> ToolResult {
    let page = writable_page(g, &p.page)?;
    let ty = p.page_type.as_deref().filter(|t| !t.trim().is_empty());
    let page = g.set_page_type(page.id, ty)?;
    Ok(match page.page_type() {
        Some(t) => format!("[[{}]] is now of type {t}\n", page.title),
        None => format!("[[{}]] has no type now\n", page.title),
    })
}

/// `set_page_tags`: replace, add and/or remove explicit tags.
pub fn set_page_tags(g: &Graph, p: &SetPageTagsParams) -> ToolResult {
    let page = writable_page(g, &p.page)?;
    let mut tags = match &p.tags {
        Some(_) => clean_list(&p.tags),
        None => page.tags.clone(),
    };
    tags.extend(clean_list(&p.add));
    let remove: HashSet<String> = clean_list(&p.remove).iter().map(|t| title_key(t)).collect();
    tags.retain(|t| !remove.contains(&title_key(t)));
    let page = g.set_page_tags(page.id, tags)?;
    Ok(if page.tags.is_empty() {
        format!("[[{}]] has no tags now\n", page.title)
    } else {
        format!("[[{}]] tags: {}\n", page.title, page.tags.join(", "))
    })
}

/// `set_page_hub`: move a page into a hub, or out of its hub.
pub fn set_page_hub(g: &Graph, p: &SetPageHubParams) -> ToolResult {
    let page = writable_page(g, &p.page)?;
    match p.hub.as_deref().filter(|h| !h.trim().is_empty()) {
        Some(h) => {
            let hub = resolve_hub(g, h)?;
            g.add_to_hub(page.id, hub.id)?;
            Ok(format!(
                "[[{}]] is now in hub [[{}]]\n",
                page.title, hub.title
            ))
        }
        None => {
            g.set_page_hubs(page.id, Vec::new())?;
            Ok(format!("[[{}]] is in no hub now\n", page.title))
        }
    }
}

/// Renders a page for `synapse://` resources.
pub fn page_resource(g: &Graph, page: &str) -> ToolResult {
    get_page(
        g,
        &GetPageParams {
            page: page.to_string(),
            ids: Some(false),
            limit: Some(2000),
            ..Default::default()
        },
    )
}
