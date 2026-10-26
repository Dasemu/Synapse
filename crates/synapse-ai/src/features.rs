//! Graph-aware AI features.
//!
//! Each feature is split in two so that no `&Graph` is held across an `.await` (the graph is
//! not `Sync`): a synchronous step reads the graph and returns a [`Job`], and
//! [`Job::run`] performs the model call and parses the result. Applying a result back to the
//! graph (`insert_tasks`, `apply_proposal`) is explicit and synchronous too, so the UI can show
//! suggestions before anything is written.

use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use synapse_core::{BlockId, Graph, NewPage, PageId, Placement, journal_title, title_key};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::client::AiClient;
use crate::error::{Error, Result};
use crate::prompts;
use crate::types::{ChatRequest, ChatResponse, TokenSink, Usage};

type Finish<O> = Box<dyn FnOnce(ChatResponse) -> Result<O> + Send>;

/// A prepared model call plus the logic that turns its reply into `O`.
pub struct Job<O> {
    request: ChatRequest,
    skip_model: bool,
    finish: Finish<O>,
}

/// Result of [`Job::run`].
#[derive(Debug, Clone)]
pub struct Outcome<O> {
    pub value: O,
    pub usage: Usage,
}

impl<O> Job<O> {
    fn new(
        request: ChatRequest,
        finish: impl FnOnce(ChatResponse) -> Result<O> + Send + 'static,
    ) -> Self {
        Self {
            request,
            skip_model: false,
            finish: Box::new(finish),
        }
    }

    /// A job that needs no model call (e.g. nothing to summarise).
    fn immediate(value: O) -> Self
    where
        O: Send + 'static,
    {
        Self {
            request: ChatRequest::default(),
            skip_model: true,
            finish: Box::new(move |_| Ok(value)),
        }
    }

    pub fn request(&self) -> &ChatRequest {
        &self.request
    }

    pub async fn run(
        self,
        ai: &AiClient,
        sink: Option<TokenSink<'_>>,
        cancel: &CancellationToken,
    ) -> Result<Outcome<O>> {
        let resp = if self.skip_model {
            ChatResponse::default()
        } else {
            ai.chat(&self.request, sink, cancel).await?
        };
        let usage = resp.usage;
        Ok(Outcome {
            value: (self.finish)(resp)?,
            usage,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// helpers

fn render_tree(graph: &Graph, page: PageId, with_ids: bool) -> Result<String> {
    let mut out = String::new();
    for (depth, b) in graph.page_tree(page)? {
        let indent = "  ".repeat(depth);
        let mut lines = b.content.lines();
        let first = lines.next().unwrap_or("");
        if with_ids {
            out.push_str(&format!("{indent}- ({}) {first}\n", b.id));
        } else {
            out.push_str(&format!("{indent}- {first}\n"));
        }
        for l in lines {
            out.push_str(&format!("{indent}  {l}\n"));
        }
    }
    Ok(out)
}

fn truncate(s: &mut String, max_chars: usize) {
    if s.chars().count() > max_chars {
        *s = s.chars().take(max_chars).collect();
        s.push_str("\n[truncated]");
    }
}

/// Rendered journals of `[from, to]` (oldest first); `None` when all are empty.
fn render_journals(
    graph: &Graph,
    from: NaiveDate,
    to: NaiveDate,
    with_ids: bool,
    max_chars: usize,
) -> Result<Option<String>> {
    let mut out = String::new();
    for page in graph.journals_between(from, to)? {
        let body = render_tree(graph, page.id, with_ids)?;
        if body.trim().is_empty() {
            continue;
        }
        let date = page.journal_date.map(journal_title).unwrap_or(page.title);
        out.push_str(&format!("<journal date=\"{date}\">\n{body}</journal>\n"));
    }
    if out.is_empty() {
        return Ok(None);
    }
    truncate(&mut out, max_chars);
    Ok(Some(out))
}

/// Extracts the first JSON object from a model reply (tolerates code fences and prose).
pub fn extract_json(text: &str) -> Result<Value> {
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else {
        return Err(Error::BadModelOutput("no JSON object in reply".into()));
    };
    serde_json::from_str(&text[a..=b]).map_err(|e| Error::BadModelOutput(e.to_string()))
}

/// All `((uuid))` references in `text`, in order, without duplicates.
pub fn parse_block_refs(text: &str) -> Vec<BlockId> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("((") {
        rest = &rest[i + 2..];
        if let Some(j) = rest.find("))")
            && let Ok(id) = Uuid::parse_str(rest[..j].trim())
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// ask_graph (RAG)

#[derive(Debug, Clone)]
pub struct AskOptions {
    pub max_blocks: usize,
    pub max_context_chars: usize,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            max_blocks: 20,
            max_context_chars: 24_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    /// Blocks the answer actually cited as `((uuid))` (only ids that were in the context).
    pub citations: Vec<BlockId>,
    /// Every block that was given to the model, most relevant first.
    pub context: Vec<BlockId>,
}

/// Retrieval-augmented question answering. `retrieve` maps the question to candidate blocks,
/// most relevant first (plug `synapse-search` in here); the context is assembled from the graph
/// within the size limits of `opts`.
pub fn ask_graph(
    graph: &Graph,
    question: &str,
    retrieve: &dyn Fn(&str) -> Vec<BlockId>,
    opts: &AskOptions,
) -> Result<Job<Answer>> {
    let mut context = Vec::new();
    let mut ctx_text = String::new();
    let mut seen = HashSet::new();
    for id in retrieve(question) {
        if context.len() >= opts.max_blocks || ctx_text.chars().count() >= opts.max_context_chars {
            break;
        }
        if !seen.insert(id) {
            continue;
        }
        let Some(block) = graph.get_block(id)? else {
            continue;
        };
        let title = graph
            .get_page(block.page_id)?
            .map(|p| p.title)
            .unwrap_or_default();
        let mut content = block.content.clone();
        truncate(&mut content, 2000);
        ctx_text.push_str(&format!(
            "<note id=\"(({id}))\" page=\"{}\">\n{content}\n</note>\n",
            title.replace('"', "'")
        ));
        context.push(id);
    }
    let user = if context.is_empty() {
        format!("Context: (no notes were found)\n\nQuestion: {question}")
    } else {
        format!("Context:\n{ctx_text}\nQuestion: {question}")
    };
    let known: HashSet<BlockId> = context.iter().copied().collect();
    Ok(Job::new(
        ChatRequest::new(prompts::ask_system(), user),
        move |r| {
            let citations = parse_block_refs(&r.text)
                .into_iter()
                .filter(|id| known.contains(id))
                .collect();
            Ok(Answer {
                text: r.text,
                citations,
                context,
            })
        },
    ))
}

// ---------------------------------------------------------------------------------------------
// summarize_journal

/// Summarises the journals of `[from, to]` (inclusive). Resolves to an empty string, without
/// calling the model, when those days have no content.
pub fn summarize_journal(graph: &Graph, from: NaiveDate, to: NaiveDate) -> Result<Job<String>> {
    let Some(text) = render_journals(graph, from, to, false, 60_000)? else {
        return Ok(Job::immediate(String::new()));
    };
    let user = format!(
        "Journals from {} to {}:\n{text}",
        journal_title(from),
        journal_title(to)
    );
    Ok(Job::new(
        ChatRequest::new(prompts::summarize_system(), user),
        |r| Ok(r.text.trim().to_string()),
    ))
}

// ---------------------------------------------------------------------------------------------
// suggest_links_and_tags

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkSuggestion {
    pub target: String,
    pub reason: String,
    /// True when `target` is an existing page; false proposes creating it.
    pub exists: bool,
    /// Block of the page where the link would fit.
    pub block: Option<BlockId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagSuggestion {
    pub tag: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Suggestions {
    pub links: Vec<LinkSuggestion>,
    pub tags: Vec<TagSuggestion>,
}

pub fn suggest_links_and_tags(graph: &Graph, page: PageId) -> Result<Job<Suggestions>> {
    let p = graph
        .get_page(page)?
        .ok_or_else(|| Error::Config(format!("page {page} not found")))?;
    let mut body = render_tree(graph, page, true)?;
    truncate(&mut body, 20_000);

    let mut titles: Vec<String> = graph.all_titles()?.into_iter().map(|(t, _)| t).collect();
    titles.sort();
    let linked: HashSet<String> = graph
        .outgoing_pages(page)?
        .into_iter()
        .filter_map(|id| graph.get_page(id).ok().flatten())
        .map(|p| title_key(&p.title))
        .collect();
    let existing: HashSet<String> = titles.iter().map(|t| title_key(t)).collect();
    let block_ids: HashSet<BlockId> = graph
        .page_tree(page)?
        .into_iter()
        .map(|(_, b)| b.id)
        .collect();
    titles.truncate(400);

    let user = format!(
        "Existing page titles:\n{}\n\nAlready linked: {}\n\n<page title=\"{}\">\n{body}</page>",
        titles.join("\n"),
        linked.iter().cloned().collect::<Vec<_>>().join(", "),
        p.title.replace('"', "'")
    );
    let own = title_key(&p.title);
    let mut req = ChatRequest::new(prompts::suggest_system(), user);
    req.json = true;
    Ok(Job::new(req, move |r| {
        let v = extract_json(&r.text)?;
        let s = |x: &Value| x.as_str().unwrap_or_default().trim().to_string();
        let mut links = Vec::new();
        for l in v["links"].as_array().into_iter().flatten() {
            let target = s(&l["target"]);
            let key = title_key(&target);
            if target.is_empty() || key == own || linked.contains(&key) {
                continue;
            }
            let block = l["block"]
                .as_str()
                .and_then(|b| Uuid::parse_str(b).ok())
                .filter(|b| block_ids.contains(b));
            links.push(LinkSuggestion {
                exists: existing.contains(&key),
                target,
                reason: s(&l["reason"]),
                block,
            });
        }
        let tags = v["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| TagSuggestion {
                tag: s(&t["tag"]).trim_start_matches('#').to_string(),
                reason: s(&t["reason"]),
            })
            .filter(|t| !t.tag.is_empty())
            .collect();
        Ok(Suggestions { links, tags })
    }))
}

// ---------------------------------------------------------------------------------------------
// extract_tasks

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSuggestion {
    pub text: String,
    pub due: Option<NaiveDate>,
    /// `A`, `B` or `C`.
    pub priority: Option<char>,
}

impl TaskSuggestion {
    /// Synapse block content: `TODO [#A] text` plus a `DEADLINE:` line when there is a due date.
    pub fn to_block_content(&self) -> String {
        let mut s = String::from("TODO ");
        if let Some(p) = self.priority {
            s.push_str(&format!("[#{p}] "));
        }
        s.push_str(&self.text);
        if let Some(d) = self.due {
            s.push_str(&format!("\nDEADLINE: <{}>", d.format("%Y-%m-%d %a")));
        }
        s
    }
}

/// Finds tasks in a block and its descendants.
pub fn extract_tasks(graph: &Graph, block: BlockId) -> Result<Job<Vec<TaskSuggestion>>> {
    let b = graph
        .get_block(block)?
        .ok_or_else(|| Error::Config(format!("block {block} not found")))?;
    let tree = graph.page_tree(b.page_id)?;
    let text = subtree_text(&tree, block);
    let today = graph.today();
    let user = format!(
        "Today is {} ({}).\n\n<note>\n{text}</note>",
        journal_title(today),
        today.format("%A")
    );
    let mut req = ChatRequest::new(prompts::tasks_system(), user);
    req.json = true;
    Ok(Job::new(req, |r| {
        let v = extract_json(&r.text)?;
        Ok(v["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let text = t["text"].as_str()?.trim().to_string();
                if text.is_empty() {
                    return None;
                }
                Some(TaskSuggestion {
                    text,
                    due: t["due"]
                        .as_str()
                        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()),
                    priority: t["priority"]
                        .as_str()
                        .and_then(|p| p.chars().next())
                        .filter(|c| matches!(c, 'A' | 'B' | 'C')),
                })
            })
            .collect())
    }))
}

fn subtree_text(tree: &[(usize, synapse_core::Block)], block: BlockId) -> String {
    let mut out = String::new();
    let Some(start) = tree.iter().position(|(_, b)| b.id == block) else {
        return out;
    };
    let base = tree[start].0;
    for (i, (depth, b)) in tree[start..].iter().enumerate() {
        if i > 0 && *depth <= base {
            break;
        }
        out.push_str(&format!("{}- {}\n", "  ".repeat(depth - base), b.content));
    }
    out
}

/// Inserts the tasks as `TODO` child blocks of `parent`.
pub fn insert_tasks(
    graph: &Graph,
    parent: BlockId,
    tasks: &[TaskSuggestion],
) -> Result<Vec<synapse_core::Block>> {
    tasks
        .iter()
        .map(|t| {
            graph
                .insert_block(
                    graph
                        .get_block(parent)?
                        .map(|b| b.page_id)
                        .ok_or_else(|| Error::Config("parent block not found".into()))?,
                    Placement::LastChildOf(Some(parent)),
                    &t.to_block_content(),
                )
                .map_err(Error::from)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// consolidate_week

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteProposal {
    pub title: String,
    /// One outline block per entry (Markdown).
    pub blocks: Vec<String>,
    pub tags: Vec<String>,
    /// Journal blocks the note draws on.
    pub sources: Vec<BlockId>,
}

/// Proposes an evergreen note from the seven journals starting at `week_start`.
/// Resolves to `None`, without calling the model, when the week has no content.
pub fn consolidate_week(graph: &Graph, week_start: NaiveDate) -> Result<Job<Option<NoteProposal>>> {
    let end = week_start + Duration::days(6);
    let Some(text) = render_journals(graph, week_start, end, true, 60_000)? else {
        return Ok(Job::immediate(None));
    };
    let valid: HashSet<BlockId> = graph
        .journals_between(week_start, end)?
        .into_iter()
        .flat_map(|p| graph.page_tree(p.id).unwrap_or_default())
        .map(|(_, b)| b.id)
        .collect();
    let user = format!(
        "Week {} (ISO week {}) to {}:\n{text}",
        journal_title(week_start),
        week_start.iso_week().week(),
        journal_title(end)
    );
    let mut req = ChatRequest::new(prompts::consolidate_system(), user);
    req.json = true;
    Ok(Job::new(req, move |r| {
        let v = extract_json(&r.text)?;
        let strings = |k: &str| -> Vec<String> {
            v[k].as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        };
        let title = v["title"].as_str().unwrap_or_default().trim().to_string();
        let blocks = strings("blocks");
        if title.is_empty() || blocks.is_empty() {
            return Err(Error::BadModelOutput(
                "proposal without title or blocks".into(),
            ));
        }
        let sources = strings("sources")
            .iter()
            .filter_map(|s| Uuid::parse_str(s.trim_matches(['(', ')'])).ok())
            .filter(|id| valid.contains(id))
            .collect();
        let tags = strings("tags")
            .into_iter()
            .map(|t| t.trim_start_matches('#').to_string())
            .collect();
        Ok(Some(NoteProposal {
            title,
            blocks,
            tags,
            sources,
        }))
    }))
}

/// Writes an accepted proposal into the graph as a new page: one block per entry, then a
/// `tags::` block and a `Sources` block with `((block))` references.
pub fn apply_proposal(graph: &Graph, p: &NoteProposal) -> Result<PageId> {
    let page = graph.create_page(NewPage {
        origin_journal: Some(graph.today()),
        ..NewPage::titled(&p.title)
    })?;
    for b in &p.blocks {
        graph.insert_block(page.id, Placement::LastChildOf(None), b)?;
    }
    if !p.tags.is_empty() {
        let tags = p
            .tags
            .iter()
            .map(|t| format!("[[{t}]]"))
            .collect::<Vec<_>>()
            .join(", ");
        graph.insert_block(
            page.id,
            Placement::LastChildOf(None),
            &format!("tags:: {tags}"),
        )?;
    }
    if !p.sources.is_empty() {
        let root = graph.insert_block(page.id, Placement::LastChildOf(None), "Sources")?;
        for s in &p.sources {
            graph.insert_block(
                page.id,
                Placement::LastChildOf(Some(root.id)),
                &format!("(({s}))"),
            )?;
        }
    }
    Ok(page.id)
}
