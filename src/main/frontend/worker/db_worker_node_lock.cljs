(ns frontend.worker.db-worker-node-lock
  "Lock file helpers for db-worker-node."
  (:require ["fs" :as fs]
            ["os" :as os]
            ["path" :as node-path]
            [clojure.string :as string]
            [frontend.worker-common.util :as worker-util]
            [lambdaisland.glogi :as log]
            [logseq.common.graph-dir :as graph-dir]
            [logseq.common.config :as common-config]
            [promesa.core :as p]))

(defn- expand-home
  [path]
  (if (string/starts-with? path "~")
    (node-path/join (.homedir os) (subs path 1))
    path))

(defn resolve-root-dir
  [root-dir]
  (expand-home (or root-dir (node-path/join (.homedir os) "logseq"))))

(defn graphs-dir
  [root-dir]
  (node-path/join (resolve-root-dir root-dir) "graphs"))

(defn repo->graph-dir-key
  [repo]
  (graph-dir/repo->graph-dir-key repo))

(defn canonical-graph-dir-key?
  [graph-dir-key]
  (and (seq graph-dir-key)
       (not (string/starts-with? graph-dir-key common-config/db-version-prefix))))

(defn decode-canonical-graph-dir-key
  [encoded-graph-dir-key]
  (let [decoded (worker-util/decode-graph-dir-name encoded-graph-dir-key)]
    (when (canonical-graph-dir-key? decoded)
      decoded)))

(defn repo-dir
  [graphs-root repo]
  (node-path/join graphs-root (worker-util/encode-graph-dir-name (repo->graph-dir-key repo))))

(defn lock-path
  [root-dir repo]
  (node-path/join (repo-dir (graphs-dir root-dir) repo) "db-worker.lock"))

(defn- pid-status
  [pid]
  (when (number? pid)
    (try
      (.kill js/process pid 0)
      :alive
      (catch :default e
        (case (.-code e)
          "ESRCH" :not-found
          "EPERM" :no-permission
          :error)))))

(def ^:private valid-owner-sources
  #{:cli :electron :unknown})

(defn normalize-owner-source
  [owner-source]
  (let [owner-source (cond
                       (keyword? owner-source) owner-source
                       (string? owner-source) (keyword owner-source)
                       :else :unknown)]
    (if (contains? valid-owner-sources owner-source)
      owner-source
      :unknown)))

(defn- normalize-lock
  [lock]
  (when lock
    (assoc lock :owner-source (normalize-owner-source (:owner-source lock)))))

(defn read-lock
  [path]
  (when (and (seq path) (fs/existsSync path))
    (normalize-lock
     (js->clj (js/JSON.parse (.toString (fs/readFileSync path) "utf8"))
              :keywordize-keys true))))

;; assert-lock-owner! runs on every write invoke (db_worker_node.cljs), so a
;; naive read-lock there means fs.existsSync + fs.readFileSync + JSON.parse
;; as a synchronous syscall on the Node event loop per request. Cached here,
;; invalidated by watching the lock file's *directory* (not the file itself
;; -- fs.watch on a path can stop firing across an unlink+recreate on some
;; platforms, watching the containing directory and filtering by filename
;; survives it) so an external change to the lock is picked up on the next
;; read instead of the cache going permanently stale (Fase 7).
;;
;; Writes this process makes itself (create-lock!/update-lock!/remove-lock!)
;; invalidate synchronously in the same call, before the fs.watch callback
;; would even fire -- the only staleness window is for a lock file changed
;; by some *other* process, on the order of a few ms until the watch
;; callback runs. That's the same window any lock-based coordination has
;; between the change happening and a reader noticing it; it does not weaken
;; the guarantee two daemons can't both hold a live lock on the same repo.
(defonce ^:private *lock-cache (atom {}))
(def ^:private absent ::absent)

(defn- invalidate-cached-lock!
  [path]
  (swap! *lock-cache update path dissoc :lock))

(defn- close-watcher!
  [path]
  (when-let [watcher (get-in @*lock-cache [path :watcher])]
    (try (.close watcher) (catch :default _ nil)))
  (swap! *lock-cache dissoc path))

(defn- ensure-watcher!
  [path]
  (when-not (get-in @*lock-cache [path :watcher])
    (try
      (let [dir (node-path/dirname path)
            file-name (node-path/basename path)
            watcher (fs/watch dir
                              (fn [_event changed-name]
                                (when (or (nil? changed-name) (= changed-name file-name))
                                  (invalidate-cached-lock! path))))]
        (swap! *lock-cache assoc-in [path :watcher] watcher))
      (catch :default e
        ;; Directory may not exist yet (lock not created yet) -- next call
        ;; retries; falls back to always-fresh reads until it succeeds.
        (log/warn :db-worker-node-lock-watch-failed {:path path :error (ex-message e)})))))

(defn- cached-lock
  [path]
  (ensure-watcher! path)
  (let [cached (get-in @*lock-cache [path :lock] absent)]
    (if (= cached absent)
      (let [lock (read-lock path)]
        (swap! *lock-cache assoc-in [path :lock] lock)
        lock)
      cached)))

(defn remove-lock!
  [path]
  (when (and (seq path) (fs/existsSync path))
    (fs/unlinkSync path))
  (close-watcher! path))

(defn create-lock!
  [{:keys [root-dir repo owner-source]}]
  (p/create
   (fn [resolve reject]
     (try
       (let [root-dir (resolve-root-dir root-dir)
             path (lock-path root-dir repo)
             existing (read-lock path)]
         (when (and existing (contains? #{:alive :no-permission} (pid-status (:pid existing))))
           (throw (ex-info "graph already locked" {:code :repo-locked :lock existing})))
         (when (and existing (= :not-found (pid-status (:pid existing))))
           (remove-lock! path))
         (fs/mkdirSync (node-path/dirname path) #js {:recursive true})
         (let [fd (fs/openSync path "wx")
               lock {:repo repo
                     :pid (.-pid js/process)
                     :lock-id (str (random-uuid))
                     :owner-source (normalize-owner-source owner-source)}]
           (try
             (fs/writeFileSync fd (js/JSON.stringify (clj->js lock)))
             (finally
               (fs/closeSync fd)))
           (invalidate-cached-lock! path)
           (resolve lock)))
       (catch :default e
         (log/error :db-worker-node-lock-create-failed e)
         (reject e))))))

(defn update-lock!
  [path lock]
  (p/create
   (fn [resolve reject]
     (try
       (let [existing (read-lock path)
             lock' (if existing
                     {:repo (:repo existing)
                      :pid (:pid existing)
                      :lock-id (or (:lock-id existing) (:lock-id lock))
                      :owner-source (normalize-owner-source (:owner-source existing))}
                     {:repo (:repo lock)
                      :pid (:pid lock)
                      :lock-id (:lock-id lock)
                      :owner-source (normalize-owner-source (:owner-source lock))})]
         (fs/mkdirSync (node-path/dirname path) #js {:recursive true})
         (fs/writeFileSync path (js/JSON.stringify (clj->js lock')))
         (invalidate-cached-lock! path)
         (resolve lock'))
       (catch :default e
         (log/error :db-worker-node-lock-update-failed e)
         (reject e))))))

(defn- lock-owner-alive?
  "The pid recorded in the lock file is, on the hot path, this very
  process's own pid -- assert-lock-owner! is the *owner* checking its own
  lock hasn't been usurped, not a stranger checking someone else's. A
  process can always signal itself, so that case needs no
  process.kill(pid, 0) syscall at all; only a genuinely foreign pid (a
  stolen/stale lock, already on the throw path) pays for the real check."
  [pid]
  (or (= pid (.-pid js/process))
      (= :alive (pid-status pid))))

(defn assert-lock-owner!
  [path {:keys [repo pid lock-id] :as owner-lock}]
  (let [lock (cached-lock path)]
    (cond
      (nil? owner-lock)
      (throw (ex-info "lock owner missing"
                      {:code :repo-locked
                       :path path}))

      (nil? lock)
      (throw (ex-info "graph lock missing"
                      {:code :repo-locked
                       :path path}))

      (not (lock-owner-alive? (:pid lock)))
      (throw (ex-info "graph lock is stale"
                      {:code :repo-locked
                       :path path
                       :lock lock}))

      (not (graph-dir/same-repo? repo (:repo lock)))
      (throw (ex-info "graph lock repo mismatch"
                      {:code :repo-locked
                       :path path
                       :lock lock
                       :owner owner-lock}))

      (not= pid (:pid lock))
      (throw (ex-info "graph lock pid mismatch"
                      {:code :repo-locked
                       :path path
                       :lock lock
                       :owner owner-lock}))

      (not= lock-id (:lock-id lock))
      (throw (ex-info "graph lock-id mismatch"
                      {:code :repo-locked
                       :path path
                       :lock lock
                       :owner owner-lock}))

      :else
      lock)))

(defn ensure-lock!
  [{:keys [root-dir repo owner-source]}]
  (let [root-dir (resolve-root-dir root-dir)
        path (lock-path root-dir repo)]
    (p/let [lock (create-lock! {:root-dir root-dir
                                :repo repo
                                :owner-source owner-source})]
      {:path path
       :lock lock})))
