(ns frontend.worker.local-tx-log
  "SQLite-backed local tx ledger, keyed by tx-id. Originally part of the
  now-deleted remote sync client (frontend.worker.sync.client-op, see Fase
  8 in docs/agent-guide); the sync-specific surface (pending-upload
  counts/checksums/graph-uuid/conflict tracking/asset-op queues) is gone,
  but two callers still need this table: db_core.cljs's built-in-property
  repair uses it as an idempotency marker (has this repo already gotten the
  repair tx?) via get-local-tx-entry/upsert-local-tx-entry!, and
  undo_redo.cljs reads a tx's recorded forward/inverse outliner ops back out
  via history-action-ops-by-tx-id so a repair tx can be undone/redone like
  any other edit."
  (:require [frontend.worker.state :as worker-state]
            [goog.object :as gobj]
            [logseq.db.sqlite.util :as sqlite-util]))

(defonce *repo->pending-local-tx-count (atom {}))

(def ^:private sqlite-schema-ready-key "__logseq_client_ops_schema_ready_v2")
(def ^:private sqlite-mode-key "__logseq_client_ops_sqlite_mode")
(def ^:private sync-meta-table-sql
  "create table if not exists sync_meta (key text primary key, value text)")
(def ^:private client-ops-table-sql
  (str "create table if not exists client_ops ("
       "id integer primary key autoincrement,"
       "kind text not null,"
       "created_at integer not null,"
       "tx_id text unique,"
       "pending integer not null default 0,"
       "failed integer not null default 0,"
       "outliner_op text,"
       "undo_redo text,"
       "forward_outliner_ops text,"
       "inverse_outliner_ops text,"
       "inferred_outliner_ops integer,"
       "normalized_tx_data text,"
       "reversed_tx_data text,"
       "asset_uuid text,"
       "asset_op text,"
       "asset_t integer,"
       "asset_value text"
       ")"))
(def ^:private pending-index-sql
  "create index if not exists idx_client_ops_pending_created on client_ops(kind, pending, created_at, id)")

(defn- client-ops-store
  [repo]
  (worker-state/get-client-ops-conn repo))

(declare ensure-sqlite-schema!)

(defn- detect-sqlite-mode
  [^js db]
  (or (gobj/get db sqlite-mode-key)
      (let [mode
            (cond
              (not db) nil
              (fn? (gobj/get db "prepare"))
              (try
                (let [^js stmt (.prepare db "select 1")]
                  (try
                    (if (fn? (gobj/get stmt "run"))
                      :better-sqlite
                      :sqlite-wasm)
                    (finally
                      (when (fn? (gobj/get stmt "finalize"))
                        (.finalize stmt)))))
                (catch :default _
                  :sqlite-wasm))
              (fn? (gobj/get db "exec")) :sqlite-wasm
              :else nil)]
        (when mode
          (try
            (gobj/set db sqlite-mode-key mode)
            (catch :default _
              nil)))
        mode)))

(defn- sqlite-db?
  [conn]
  (some? (detect-sqlite-mode conn)))

(defn- sqlite-store-or-throw
  [repo]
  (when-let [store (client-ops-store repo)]
    (if (sqlite-db? store)
      (do
        (ensure-sqlite-schema! store)
        store)
      (throw (ex-info "Legacy DataScript client-op storage is unsupported. Please back up the graph and re-download it."
                      {:type :db-sync/legacy-client-ops-storage
                       :repo repo})))))

(defn- parse-uuid-str
  [v]
  (when (string? v)
    (try
      (uuid v)
      (catch :default _
        nil))))

(defn- kw->str
  [v]
  (cond
    (keyword? v) (name v)
    (string? v) v
    :else nil))

(defn- str->kw
  [v]
  (when (string? v)
    (keyword v)))

(defn- bool->int [v] (if v 1 0))
(defn- int->bool [v] (not (or (nil? v) (= 0 v) (= false v))))

(defn- normalize-op-entries
  [ops]
  (let [ops' (some-> ops seq vec)]
    (cond
      (nil? ops')
      nil

      (and (keyword? (first ops'))
           (vector? (second ops')))
      [ops']

      :else
      ops')))

(defn- sqlite-run!
  [^js db sql params]
  (case (detect-sqlite-mode db)
    :better-sqlite
    (let [^js stmt (.prepare db sql)
          run-fn (gobj/get stmt "run")]
      (if (seq params)
        (.apply run-fn stmt (to-array params))
        (.call run-fn stmt)))

    :sqlite-wasm
    (.exec db #js {:sql sql
                   :bind (into-array params)})

    nil))

(defn- sqlite-rows
  [^js db sql params]
  (case (detect-sqlite-mode db)
    :better-sqlite
    (let [^js stmt (.prepare db sql)
          all-fn (gobj/get stmt "all")]
      (vec (if (seq params)
             (.apply all-fn stmt (to-array params))
             (.call all-fn stmt))))

    :sqlite-wasm
    (let [^js result (.exec db #js {:sql sql
                                :bind (into-array params)
                                :rowMode "object"
                                :returnValue "resultRows"})]
      (cond
        (nil? result) []
        (array? result) (vec result)
        (fn? (gobj/get result "toArray")) (vec (.toArray result))
        :else []))

    []))

(defn- sqlite-row
  [db sql params]
  (first (sqlite-rows db sql params)))

(defn- sqlite-with-tx!
  [^js db f]
  (case (detect-sqlite-mode db)
    :better-sqlite
    (let [tx-fn (.transaction db (fn [] (f db)))]
      (tx-fn))

    :sqlite-wasm
    (if (fn? (gobj/get db "transaction"))
      (.transaction db (fn [tx] (f tx)))
      (f db))

    (f db)))

(defn ensure-sqlite-schema!
  [db]
  (when (sqlite-db? db)
    (when-not (true? (gobj/get db sqlite-schema-ready-key))
      (sqlite-with-tx!
       db
       (fn [tx]
         (sqlite-run! tx sync-meta-table-sql [])
         (sqlite-run! tx client-ops-table-sql [])
         (sqlite-run! tx pending-index-sql [])))
      (try
        (gobj/set db sqlite-schema-ready-key true)
        (catch :default _
          nil)))))

(defn- sqlite-get-meta
  [db k]
  (some-> (sqlite-row db "select value from sync_meta where key = ?" [(name k)])
          (aget "value")))

(defn- sqlite-set-meta!
  [db k v]
  (sqlite-run! db
               (str "insert into sync_meta (key, value) values (?, ?)"
                    " on conflict(key) do update set value = excluded.value")
               [(name k) (str v)]))

(defn get-local-tx
  [repo]
  (when-let [store (sqlite-store-or-throw repo)]
    (when-let [result (sqlite-get-meta store :local-tx)]
      (js/parseInt result 10))))

(defn update-local-tx
  [repo t]
  {:pre [(and (integer? t) (>= t 0))]}
  (let [store (sqlite-store-or-throw repo)
        prev-t (get-local-tx repo)]
    (assert (some? store) repo)
    (when (and prev-t (< t prev-t))
      (throw (ex-info "local-tx should be monotonically increasing"
                      {:repo repo
                       :prev-t prev-t
                       :new-t t})))
    (sqlite-set-meta! store :local-tx t)))

(defn- row->pending-local-tx
  [row]
  (let [tx-id (parse-uuid-str (aget row "tx_id"))]
    (when tx-id
      {:tx-id tx-id
       :outliner-op (str->kw (aget row "outliner_op"))
       :forward-outliner-ops (or (normalize-op-entries
                                  (sqlite-util/read-transit-str (aget row "forward_outliner_ops")))
                                 [])
       :inverse-outliner-ops (or (normalize-op-entries
                                  (sqlite-util/read-transit-str (aget row "inverse_outliner_ops")))
                                 [])
       :inferred-outliner-ops? (int->bool (aget row "inferred_outliner_ops"))
       :db-sync/undo-redo (str->kw (aget row "undo_redo"))
       :tx (sqlite-util/read-transit-str (aget row "normalized_tx_data"))
       :reversed-tx (sqlite-util/read-transit-str (aget row "reversed_tx_data"))})))

(defn upsert-local-tx-entry!
  [repo {:keys [tx-id created-at pending? failed? outliner-op undo-redo
                forward-outliner-ops inverse-outliner-ops inferred-outliner-ops?
                normalized-tx-data reversed-tx-data]
         :or {pending? true failed? false}}]
  {:pre [(some? tx-id)]}
  (let [store (sqlite-store-or-throw repo)]
    (assert (some? store) repo)
    (let [tx-id-str (str tx-id)
          existing (sqlite-row store
                               "select pending, created_at from client_ops where kind = 'tx' and tx_id = ?"
                               [tx-id-str])
          should-inc-pending? (not= 1 (some-> existing (aget "pending")))
          forward-outliner-ops' (or (normalize-op-entries forward-outliner-ops) [])
          inverse-outliner-ops' (or (normalize-op-entries inverse-outliner-ops) [])
          created-at' (or (some-> existing (aget "created_at"))
                          created-at
                          (.now js/Date))]
      (sqlite-run! store
                   (str "insert into client_ops ("
                        "kind, created_at, tx_id, pending, failed, outliner_op, undo_redo, "
                        "forward_outliner_ops, inverse_outliner_ops, inferred_outliner_ops, "
                        "normalized_tx_data, reversed_tx_data"
                        ") values ('tx', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
                        " on conflict(tx_id) do update set "
                        "created_at = excluded.created_at,"
                        "pending = excluded.pending,"
                        "failed = excluded.failed,"
                        "outliner_op = excluded.outliner_op,"
                        "undo_redo = excluded.undo_redo,"
                        "forward_outliner_ops = excluded.forward_outliner_ops,"
                        "inverse_outliner_ops = excluded.inverse_outliner_ops,"
                        "inferred_outliner_ops = excluded.inferred_outliner_ops,"
                        "normalized_tx_data = excluded.normalized_tx_data,"
                        "reversed_tx_data = excluded.reversed_tx_data")
                   [created-at'
                    tx-id-str
                    (bool->int pending?)
                    (bool->int failed?)
                    (kw->str outliner-op)
                    (kw->str undo-redo)
                    (sqlite-util/write-transit-str forward-outliner-ops')
                    (sqlite-util/write-transit-str inverse-outliner-ops')
                    (bool->int inferred-outliner-ops?)
                    (sqlite-util/write-transit-str (or normalized-tx-data []))
                    (sqlite-util/write-transit-str (or reversed-tx-data []))])
      {:tx-id tx-id
       :created-at created-at'
       :should-inc-pending? should-inc-pending?})))

(defn get-local-tx-entry
  [repo tx-id]
  (when (uuid? tx-id)
    (when-let [store (sqlite-store-or-throw repo)]
      (some-> (sqlite-row store
                          (str "select tx_id, outliner_op, undo_redo, "
                               "forward_outliner_ops, inverse_outliner_ops, inferred_outliner_ops, "
                               "normalized_tx_data, reversed_tx_data "
                               "from client_ops where kind = 'tx' and tx_id = ? limit 1")
                          [(str tx-id)])
              (row->pending-local-tx)))))

(defn adjust-pending-local-tx-count!
  [repo delta]
  (swap! *repo->pending-local-tx-count
         (fn [m]
           (let [base (or (get m repo) 0)
                 next (max 0 (+ base delta))]
             (assoc m repo next)))))

(defn history-action-ops-by-tx-id
  [repo tx-id]
  (when-let [entry (get-local-tx-entry repo tx-id)]
    {:db-sync/forward-outliner-ops (some-> (:forward-outliner-ops entry) seq vec)
     :db-sync/inverse-outliner-ops (some-> (:inverse-outliner-ops entry) seq vec)}))

(defn cleanup-finished-history-ops!
  [repo protected-tx-ids]
  (if-let [store (sqlite-store-or-throw repo)]
    (let [protected-tx-ids (set protected-tx-ids)
          tx-id-rows (sqlite-rows store
                                  (str "select tx_id from client_ops "
                                       "where kind = 'tx' and pending = 0 and tx_id is not null")
                                  [])
          removable-tx-ids (->> tx-id-rows
                                (keep (fn [row]
                                        (let [tx-id (parse-uuid-str (aget row "tx_id"))]
                                          (when (and (uuid? tx-id)
                                                     (not (contains? protected-tx-ids tx-id)))
                                            tx-id))))
                                vec)]
      (when (seq removable-tx-ids)
        (doseq [tx-id removable-tx-ids]
          (sqlite-run! store
                       "delete from client_ops where kind = 'tx' and tx_id = ?"
                       [(str tx-id)])))
      (count removable-tx-ids))
    0))
