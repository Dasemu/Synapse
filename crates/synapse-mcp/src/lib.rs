//! MCP (Model Context Protocol) server over a Synapse graph. See `docs/mcp.md`.
//!
//! * [`tools`]: the tool implementations, plain functions over a [`Graph`] returning compact
//!   Markdown (testable without MCP).
//! * [`SynapseMcp`]: the rmcp handler (tool list/schemas, resources, dispatch).
//! * [`GraphHost`]: how a call reaches the graph. [`LocalHost`] opens a local graph
//!   directory per call (stdio binary, next to the running app); the sync server provides
//!   its own host over its shared graphs and exposes the handler over Streamable HTTP.
//!
//! Read tools run inside a transaction that is always rolled back, so they can never change
//! the graph. Write tools run in `BEGIN IMMEDIATE` (short write lock, retried while SQLite is
//! busy) through the normal `Graph` API, so every change is logged as ops and syncs.
//! Journals are read-only: write tools refuse journal pages.

pub mod params;
pub mod text;
pub mod tools;

use params::*;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ListResourceTemplatesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents,
    ResourceTemplate, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use synapse_core::Graph;
use tools::{ToolError, ToolResult};

/// A unit of work on the graph (runs on a blocking thread; may be retried when busy).
pub type Job = Box<dyn Fn(&Graph) -> ToolResult + Send + Sync + 'static>;
pub type HostFuture = Pin<Box<dyn Future<Output = ToolResult> + Send + 'static>>;

/// Gives tool calls access to a graph.
pub trait GraphHost: Send + Sync + 'static {
    /// Runs `job` with exclusive use of the graph's connection.
    fn run(&self, job: Job) -> HostFuture;
}

/// The host for this request. The HTTP server puts it in the request extensions (one per
/// graph); stdio sets it on the handler.
#[derive(Clone)]
pub struct HostRef(pub Arc<dyn GraphHost>);

/// A local graph directory, opened per call so this process never keeps state (or an old
/// clock) next to the app that owns the graph. SQLite WAL lets both use it at once.
pub struct LocalHost {
    dir: PathBuf,
}

impl LocalHost {
    /// Fails when `dir` holds no graph (never creates one).
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        if !dir.join("synapse.db").is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no Synapse graph in {} (synapse.db missing)", dir.display()),
            ));
        }
        Ok(Self { dir })
    }
}

impl GraphHost for LocalHost {
    fn run(&self, job: Job) -> HostFuture {
        let dir = self.dir.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let g = Graph::open(&dir).map_err(ToolError::from)?;
                job(&g)
            })
            .await
            .map_err(|e| ToolError::new(format!("internal error: {e}")))?
        })
    }
}

/// Rolls back on drop unless committed.
struct Txn<'a> {
    g: &'a Graph,
    open: bool,
}

impl<'a> Txn<'a> {
    fn begin(g: &'a Graph, sql: &str) -> Result<Self, ToolError> {
        g.conn().execute_batch(sql)?;
        Ok(Self { g, open: true })
    }
    fn commit(mut self) -> Result<(), ToolError> {
        self.open = false;
        self.g.conn().execute_batch("COMMIT")?;
        Ok(())
    }
}

impl Drop for Txn<'_> {
    fn drop(&mut self) {
        if self.open {
            let _ = self.g.conn().execute_batch("ROLLBACK");
        }
    }
}

const BUSY_RETRIES: usize = 3;

fn with_retry(g: &Graph, f: impl Fn(&Graph) -> ToolResult) -> ToolResult {
    let mut attempt = 0;
    loop {
        match f(g) {
            Err(e) if e.busy && attempt < BUSY_RETRIES => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(200 * attempt as u64));
            }
            other => return other,
        }
    }
}

/// Wraps a read: a deferred transaction (consistent snapshot) that is always rolled back.
pub fn read_job(f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static) -> Job {
    Box::new(move |g| {
        with_retry(g, |g| {
            let _tx = Txn::begin(g, "BEGIN")?;
            f(g)
        })
    })
}

/// Wraps a write: `BEGIN IMMEDIATE` (takes the write lock up front, waits while busy),
/// committed only when the tool succeeds.
pub fn write_job(f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static) -> Job {
    Box::new(move |g| {
        with_retry(g, |g| {
            let tx = Txn::begin(g, "BEGIN IMMEDIATE")?;
            let out = f(g)?;
            tx.commit()?;
            Ok(out)
        })
    })
}

const INSTRUCTIONS: &str = "Synapse is an outliner notes graph (Logseq-like): pages made of \
nested Markdown blocks, daily journals (pages titled YYYY-MM-DD), [[links]], #tags, one optional \
type per page, and hubs (folders). Pages are addressed by title, alias or id; blocks by the \
((id)) shown after them. Typical workflow: read recent journals (get_journal, journal_links, \
timeline), then curate pages: create pages for things mentioned in journals, type and tag them \
reusing the existing vocabulary (list_types, list_tags) and file them in hubs. Journals are \
read-only through this server.";

/// The MCP handler. Cheap to clone.
#[derive(Clone)]
pub struct SynapseMcp {
    host: Option<Arc<dyn GraphHost>>,
    allow_write: bool,
    tool_router: ToolRouter<Self>,
}

fn reply(r: ToolResult) -> Result<CallToolResult, ErrorData> {
    Ok(match r {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.message)]),
    })
}

impl SynapseMcp {
    /// `host`: the graph for every call (stdio), or `None` to take it from the HTTP request
    /// extensions ([`HostRef`]). Write tools are only listed and callable with `allow_write`.
    pub fn new(host: Option<Arc<dyn GraphHost>>, allow_write: bool) -> Self {
        let mut tool_router = Self::read_tools();
        if allow_write {
            tool_router.merge(Self::write_tools());
        }
        Self {
            host,
            allow_write,
            tool_router,
        }
    }

    fn host_for(&self, ctx: &RequestContext<RoleServer>) -> Result<Arc<dyn GraphHost>, ToolError> {
        if let Some(h) = &self.host {
            return Ok(h.clone());
        }
        ctx.extensions
            .get::<http::request::Parts>()
            .and_then(|p| p.extensions.get::<HostRef>())
            .map(|h| h.0.clone())
            .ok_or_else(|| ToolError::new("no graph bound to this request"))
    }

    async fn read(
        &self,
        ctx: &RequestContext<RoleServer>,
        f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        reply(match self.host_for(ctx) {
            Ok(h) => h.run(read_job(f)).await,
            Err(e) => Err(e),
        })
    }

    async fn write(
        &self,
        ctx: &RequestContext<RoleServer>,
        f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        if !self.allow_write {
            return reply(Err(ToolError::new("this server is read-only")));
        }
        reply(match self.host_for(ctx) {
            Ok(h) => h.run(write_job(f)).await,
            Err(e) => Err(e),
        })
    }
}

#[tool_router(router = read_tools, vis = "")]
impl SynapseMcp {
    #[tool(
        description = "Full-text search over page titles/aliases and block text (all words must match; case- and accent-insensitive). Returns [[page]] ((block-id)) date: snippet. Filters: scope, journals, tag, type, hub, page, from/to dates.",
        annotations(read_only_hint = true)
    )]
    async fn search(
        &self,
        Parameters(p): Parameters<SearchParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::search(g, &p)).await
    }

    #[tool(
        description = "A page (by title, alias, id or journal date) with its type, tags, hub, aliases, properties and its outline as indented Markdown; each block ends with its ((id)). Use `block` for a subtree, `max_depth` to skim, cursor for long pages. Hubs list their members.",
        annotations(read_only_hint = true)
    )]
    async fn get_page(
        &self,
        Parameters(p): Parameters<GetPageParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::get_page(g, &p)).await
    }

    #[tool(
        description = "Journal outline for a day (YYYY-MM-DD, \"today\", \"yesterday\"; default today) or every journal in a from/to range (max 31 days, oldest first).",
        annotations(read_only_hint = true)
    )]
    async fn get_journal(
        &self,
        Parameters(p): Parameters<GetJournalParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::get_journal(g, &p)).await
    }

    #[tool(
        description = "List pages with filters (type, tag, hub, untyped, untagged, updated_since, created_since, journals exclude/include/only), sorted by updated, created or title, paginated with a cursor.",
        annotations(read_only_hint = true)
    )]
    async fn list_pages(
        &self,
        Parameters(p): Parameters<ListPagesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::list_pages(g, &p)).await
    }

    #[tool(
        description = "Linked references to a page: the blocks that link it, grouped by source page, with their parent block as context.",
        annotations(read_only_hint = true)
    )]
    async fn backlinks(
        &self,
        Parameters(p): Parameters<PageRefParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::backlinks(g, &p)).await
    }

    #[tool(
        description = "Pages related to a page by spreading activation over links, co-visits and time (strongest first).",
        annotations(read_only_hint = true)
    )]
    async fn related(
        &self,
        Parameters(p): Parameters<PageRefParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::related(g, &p)).await
    }

    #[tool(
        description = "Tags in use with page counts. Reuse these names when tagging.",
        annotations(read_only_hint = true)
    )]
    async fn list_tags(
        &self,
        Parameters(p): Parameters<ListParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::list_tags(g, &p)).await
    }

    #[tool(
        description = "Page types in use with counts and icons, then defined/suggested unused ones. Reuse these names when typing pages.",
        annotations(read_only_hint = true)
    )]
    async fn list_types(
        &self,
        Parameters(p): Parameters<ListParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::list_types(g, &p)).await
    }

    #[tool(
        description = "The hub (folder) tree with note counts, or the members of one hub.",
        annotations(read_only_hint = true)
    )]
    async fn hubs(
        &self,
        Parameters(p): Parameters<HubsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::hubs(g, &p)).await
    }

    #[tool(
        description = "Per day between from and to (default last 7 days, max 92): whether a journal exists, pages born that day and pages edited.",
        annotations(read_only_hint = true)
    )]
    async fn timeline(
        &self,
        Parameters(p): Parameters<RangeParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::timeline(g, &p)).await
    }

    #[tool(
        description = "Curation helper: per journal day (default last 7 days, max 31), the pages it links with their status (type, tags, hub, untyped, untagged, empty) and existing pages it mentions without linking.",
        annotations(read_only_hint = true)
    )]
    async fn journal_links(
        &self,
        Parameters(p): Parameters<RangeParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.read(&ctx, move |g| tools::journal_links(g, &p)).await
    }
}

#[tool_router(router = write_tools, vis = "")]
impl SynapseMcp {
    #[tool(
        description = "Create a page with optional type, tags, hub and a Markdown outline body. `source_journal` records the journal day it comes from. Fails if the title exists.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn create_page(
        &self,
        Parameters(p): Parameters<CreatePageParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::create_page(g, &p)).await
    }

    #[tool(
        description = "Append a Markdown outline to a page (at the end, or under a parent block). Journals are read-only.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn append_blocks(
        &self,
        Parameters(p): Parameters<AppendBlocksParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::append_blocks(g, &p)).await
    }

    #[tool(
        description = "Replace the text of a block on a page (children are kept). Journal blocks are read-only.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn update_block(
        &self,
        Parameters(p): Parameters<UpdateBlockParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::update_block(g, &p)).await
    }

    #[tool(
        description = "Delete a block and its children from a page. Journal blocks are read-only.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn delete_block(
        &self,
        Parameters(p): Parameters<DeleteBlockParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::delete_block(g, &p)).await
    }

    #[tool(
        description = "Set (or clear with null) the single type of a page.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn set_page_type(
        &self,
        Parameters(p): Parameters<SetPageTypeParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::set_page_type(g, &p)).await
    }

    #[tool(
        description = "Change a page's explicit tags: `tags` replaces them all; `add` / `remove` edit them.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn set_page_tags(
        &self,
        Parameters(p): Parameters<SetPageTagsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::set_page_tags(g, &p)).await
    }

    #[tool(
        description = "Move a page into a hub (a page has at most one), or out of its hub with null.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn set_page_hub(
        &self,
        Parameters(p): Parameters<SetPageHubParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.write(&ctx, move |g| tools::set_page_hub(g, &p)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SynapseMcp {
    fn get_info(&self) -> ServerConfig {
        let mut instructions = INSTRUCTIONS.to_string();
        if !self.allow_write {
            instructions.push_str(" This server is read-only.");
        }
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "synapse-mcp",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(instructions)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let t = |uri: &str, name: &str, desc: &str| {
            ResourceTemplate::new(uri, name)
                .with_description(desc)
                .with_mime_type("text/markdown")
        };
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            t(
                "synapse://page/{title}",
                "page",
                "A page (title, alias or id) as a Markdown outline",
            ),
            t(
                "synapse://journal/{date}",
                "journal",
                "A journal day (YYYY-MM-DD or today) as a Markdown outline",
            ),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let target = if let Some(t) = uri.strip_prefix("synapse://page/") {
            percent_decode(t)
        } else if let Some(d) = uri.strip_prefix("synapse://journal/") {
            percent_decode(d)
        } else {
            return Err(ErrorData::invalid_params(
                format!("unknown resource {uri}"),
                None,
            ));
        };
        let host = self
            .host_for(&ctx)
            .map_err(|e| ErrorData::internal_error(e.message, None))?;
        let text = host
            .run(read_job(move |g| tools::page_resource(g, &target)))
            .await
            .map_err(|e| ErrorData::invalid_params(e.message, None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("text/markdown"),
        ])
        .into())
    }
}

/// Decodes `%XX` escapes (titles in resource URIs).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
