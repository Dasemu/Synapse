//! Server-side graphs: one `synapse_core::Graph` per `SYNAPSE_DATA_DIR/<graph_id>/`.

use crate::error::{ApiError, ApiResult};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use synapse_core::Graph;
use synapse_core::blob::BlobStore;
use synapse_sync::protocol::is_valid_graph_id;
use tokio::sync::{Mutex as AsyncMutex, watch};

pub struct GraphHandle {
    pub dir: PathBuf,
    pub server_id: String,
    pub blobs: BlobStore,
    graph: Mutex<Graph>,
    /// Latest op seq; subscribers (SSE) are woken when it changes.
    pub seq: watch::Sender<i64>,
    /// Hashes with an upload in flight.
    pub uploads: Mutex<HashSet<String>>,
}

impl GraphHandle {
    /// Runs `f` on the blocking pool with the graph locked (the core `Graph` is `!Sync` and
    /// SQLite is synchronous).
    pub async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Graph) -> synapse_core::Result<T> + Send + 'static,
    ) -> ApiResult<T> {
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let g = me.graph.lock().unwrap_or_else(PoisonError::into_inner);
            f(&g)
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::from)
    }

    pub fn publish_seq(&self, seq: i64) {
        self.seq.send_if_modified(|cur| {
            if seq > *cur {
                *cur = seq;
                true
            } else {
                false
            }
        });
    }
}

pub struct Graphs {
    root: PathBuf,
    map: AsyncMutex<HashMap<String, Arc<GraphHandle>>>,
}

impl Graphs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            map: AsyncMutex::new(HashMap::new()),
        }
    }

    /// Returns an existing graph (`None` if it was never created).
    pub async fn get(&self, id: &str) -> ApiResult<Option<Arc<GraphHandle>>> {
        self.open(id, false).await
    }

    /// Returns the graph, creating it on first use.
    pub async fn get_or_create(&self, id: &str) -> ApiResult<Arc<GraphHandle>> {
        Ok(self.open(id, true).await?.expect("created"))
    }

    async fn open(&self, id: &str, create: bool) -> ApiResult<Option<Arc<GraphHandle>>> {
        if !is_valid_graph_id(id) {
            return Err(ApiError::bad_request("invalid graph id"));
        }
        let mut map = self.map.lock().await;
        if let Some(h) = map.get(id) {
            return Ok(Some(h.clone()));
        }
        let dir = self.root.join(id);
        if !create && !dir.join("synapse.db").exists() {
            return Ok(None);
        }
        let handle = tokio::task::spawn_blocking(move || -> synapse_core::Result<GraphHandle> {
            let graph = Graph::open(&dir)?;
            let seq = graph.last_seq()?;
            Ok(GraphHandle {
                server_id: graph.actor().to_string(),
                blobs: graph.blobs().clone(),
                dir,
                graph: Mutex::new(graph),
                seq: watch::channel(seq).0,
                uploads: Mutex::new(HashSet::new()),
            })
        })
        .await
        .map_err(ApiError::internal)??;
        let handle = Arc::new(handle);
        map.insert(id.to_string(), handle.clone());
        Ok(Some(handle))
    }
}
