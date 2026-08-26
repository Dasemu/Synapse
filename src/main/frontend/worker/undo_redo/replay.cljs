(ns frontend.worker.undo-redo.replay
  "Local undo/redo history recording and replay.

  Ported from the now-deleted frontend.worker.sync.apply-txs (Fase 8 in
  docs/agent-guide), which mixed this together with the remote-sync tx
  upload/apply protocol. This is the part that survives: every local
  outliner edit gets its forward/inverse outliner ops derived and stashed in
  frontend.worker.local-tx-log (keyed by tx-id) via persist-local-tx!, then
  immediately read back and pushed onto the in-memory undo/redo stack by
  worker-undo-redo/gen-undo-ops! -- the local-tx-log row is a short-lived
  relay, not a source of truth (the undo-stack entry embeds the ops
  directly), so it's safe for db_core.cljs's periodic
  cleanup-finished-history-ops! to reclaim it later. apply-history-action! is
  the reverse direction: it's what actually runs when the user hits
  undo/redo, replaying an entry's forward or inverse outliner ops (or, for
  edits that bypassed the structured outliner-op layer entirely, a raw
  normalized/reversed tx-data fallback) against the *current* db state --
  this is registered as worker-undo-redo/*apply-history-action!, the
  extension point undo_redo.cljs calls on every undo/redo."
  (:require [datascript.core :as d]
            [frontend.worker.local-tx-log :as local-tx-log]
            [frontend.worker.state :as worker-state]
            [frontend.worker.undo-redo :as worker-undo-redo]
            [lambdaisland.glogi :as log]
            [logseq.db :as ldb]
            [logseq.db.common.entity-plus :as entity-plus]
            [logseq.db.common.normalize :as db-normalize]
            [logseq.outliner.core :as outliner-core]
            [logseq.outliner.op :as outliner-op]
            [logseq.outliner.op.construct :as op-construct]
            [logseq.outliner.page :as outliner-page]
            [logseq.outliner.property :as outliner-property]
            [logseq.outliner.recycle :as outliner-recycle]))

;; ---- recording: derive + stash + push onto the undo stack ----

(defn- tx-meta-outliner-op
  [tx-meta]
  (or (:outliner-op tx-meta)
      (when (:db-migrate? tx-meta)
        :db-migrate)))

(defn- inferred-outliner-ops?
  [tx-meta]
  (and (nil? (:outliner-ops tx-meta))
       (not (:undo? tx-meta))
       (not (:redo? tx-meta))
       (not= :batch-import-edn (:outliner-op tx-meta))))

(defn- normalize-tx-data [db-after db-before tx-data]
  (db-normalize/normalize-tx-data db-after db-before tx-data))

(defn- reverse-tx-data [db-before db-after tx-data]
  (->> tx-data
       reverse
       (keep (fn [[e a v t added]]
               (let [reversed-datom (d/datom e a v t (not added))]
                 ;; trick: reverse the order of `db-before` and `db-after`
                 (db-normalize/normalize-datom db-before db-after reversed-datom))))
       (db-normalize/replace-attr-retract-with-retract-entity-v2 db-after)
       db-normalize/reorder-retract-entity))

(declare apply-history-action!)

(defn persist-local-tx!
  "Runs on every committed local tx (see db_listener.cljs's :persist-local-tx
  handler): derives this tx's forward/inverse outliner ops, stashes them in
  local-tx-log keyed by tx-id, then hands off to gen-undo-ops! to push the
  undo-stack entry (and, the first time, register apply-history-action! as
  the default replay implementation -- same lazy-registration pattern the
  deleted sync code used). A no-op when the graph has no client-ops conn
  open (e.g. :publishing)."
  [repo {:keys [db-before db-after tx-data tx-meta] :as tx-report}]
  (when (and (worker-state/get-client-ops-conn repo)
             (:persist-op? tx-meta true))
    (let [tx-id (or (:db-sync/tx-id tx-meta) (random-uuid))
          outliner-op (tx-meta-outliner-op tx-meta)
          {:keys [forward-outliner-ops inverse-outliner-ops]}
          (op-construct/derive-history-outliner-ops db-before db-after tx-data tx-meta)
          normalized-tx-data (normalize-tx-data db-after db-before tx-data)
          reversed-datoms (reverse-tx-data db-before db-after tx-data)]
      (local-tx-log/upsert-local-tx-entry!
       repo
       {:tx-id tx-id
        :created-at (.now js/Date)
        ;; No upload queue to mark this false later -- write it already
        ;; settled so db_core.cljs's periodic cleanup-finished-history-ops!
        ;; (which only reclaims pending = 0 rows, protecting whatever the
        ;; live undo/redo stack still references) is the sole garbage
        ;; collector for this table.
        :pending? false
        :failed? false
        :outliner-op outliner-op
        :undo-redo (cond
                     (:undo? tx-meta) :undo
                     (:redo? tx-meta) :redo
                     :else :none)
        :forward-outliner-ops forward-outliner-ops
        :inverse-outliner-ops inverse-outliner-ops
        :inferred-outliner-ops? (inferred-outliner-ops? tx-meta)
        :normalized-tx-data normalized-tx-data
        :reversed-tx-data reversed-datoms})
      (worker-undo-redo/gen-undo-ops! repo tx-report tx-id
                                      {:apply-history-action! apply-history-action!})
      tx-id)))

;; ---- replay: apply a recorded entry's ops against the current db ----

(defn- inline-history-action
  [tx-meta]
  (let [forward-outliner-ops (or (:db-sync/forward-outliner-ops tx-meta)
                                 (:forward-outliner-ops tx-meta))
        inverse-outliner-ops (or (:db-sync/inverse-outliner-ops tx-meta)
                                 (:inverse-outliner-ops tx-meta))]
    (when (and (seq forward-outliner-ops) (seq inverse-outliner-ops))
      {:outliner-op (:outliner-op tx-meta)
       :forward-outliner-ops forward-outliner-ops
       :inverse-outliner-ops inverse-outliner-ops})))

(defn- normalize-tx-data-for-rebase
  [tx-data]
  (some->> tx-data
           (mapv (fn [item]
                   (if (and (vector? item) (= 5 (count item)))
                     (let [[op e a v _t] item]
                       [op e a v])
                     item)))
           db-normalize/reorder-retract-entity))

(defn- expected-history-action-error-reason?
  [reason]
  (contains? #{:invalid-history-action-ops
               :invalid-history-action-tx}
             reason))

(defn- history-action-error-reason
  [error]
  (let [message (ex-message error)]
    (if (or (= "invalid rebase op" message)
            (= "Non-transact outliner ops contain numeric entity ids" message))
      :invalid-history-action-ops
      :error)))

(defn- invalid-rebase-op!
  [op data]
  (throw (ex-info "invalid rebase op" (assoc data :op op))))

(defn- replay-entity-id-value
  [db v]
  (cond
    (number? v)
    v

    (uuid? v)
    (some-> (d/entity db [:block/uuid v]) :db/id)

    (or (vector? v) (qualified-keyword? v))
    (some-> (d/entity db v) :db/id)

    :else
    v))

(defn- replay-entity-id-coll
  [db ids]
  (mapv #(or (replay-entity-id-value db %) %) ids))

(defn- rebase-find-existing-left-sibling
  [current-db target]
  (loop [sibling (ldb/get-left-sibling target)]
    (if (nil? sibling)
      nil
      (if-let [current-sibling (and sibling (d/entity current-db [:block/uuid (:block/uuid sibling)]))]
        current-sibling
        (recur (ldb/get-left-sibling sibling))))))

(defn- rebase-target-ref
  [target-id]
  (cond
    (and (vector? target-id)
         (= :block/uuid (first target-id))
         (uuid? (second target-id)))
    target-id

    (uuid? target-id)
    [:block/uuid target-id]

    (and (map? target-id) (uuid? (:block/uuid target-id)))
    [:block/uuid (:block/uuid target-id)]

    :else
    target-id))

(defn- rebase-resolve-target-and-sibling
  [current-db rebase-db-before target-id sibling?]
  (let [target-ref (rebase-target-ref target-id)
        target (d/entity current-db target-ref)
        target-before (when rebase-db-before
                        (d/entity rebase-db-before target-ref))
        parent-before (when rebase-db-before
                        (:block/parent (d/entity rebase-db-before target-ref)))]
    (cond
      target
      [target sibling?]

      (and target-before parent-before sibling?)
      (if-let [left-sibling (rebase-find-existing-left-sibling current-db target-before)]
        [left-sibling true]
        (when-let [parent (d/entity current-db [:block/uuid (:block/uuid parent-before)])]
          [parent false]))

      :else
      nil)))

(defn- template-parent-ref
  [parent]
  (cond
    (and (vector? parent) (= :block/uuid (first parent)))
    parent

    (uuid? parent)
    [:block/uuid parent]

    (and (map? parent) (uuid? (:block/uuid parent)))
    [:block/uuid (:block/uuid parent)]

    :else
    parent))

(defn- sanitize-template-block
  [current-db rebase-db-before block]
  (let [m (into {} block)
        block-id (:db/id m)
        block-uuid (or (:block/uuid m)
                       (when (number? block-id)
                         (or (some-> rebase-db-before (d/entity block-id) :block/uuid)
                             (some-> (d/entity current-db block-id) :block/uuid)))
                       (when (and (vector? block-id)
                                  (= :block/uuid (first block-id))
                                  (uuid? (second block-id)))
                         (second block-id)))]
    (cond-> (-> m
                (dissoc :db/id :block/order :block/page :block/tx-id)
                (update :block/parent template-parent-ref))
      (uuid? block-uuid)
      (assoc :block/uuid block-uuid))))

(defn- block-ref?
  [v]
  (or (number? v)
      (and (vector? v)
           (= :block/uuid (first v))
           (uuid? (second v)))))

(defn- block-entity
  [db ref]
  (when (block-ref? ref)
    (d/entity db ref)))

(defn- block-entity-ref
  [entity]
  (if-let [block-uuid (:block/uuid entity)]
    [:block/uuid block-uuid]
    (:db/id entity)))

(defn- block-descendants
  [entity]
  (letfn [(collect [block]
            (mapcat (fn [child]
                      (cons child (collect child)))
                    (sort-by :block/order
                             (entity-plus/lookup-kv-then-entity block :block/_raw-parent))))]
    (collect entity)))

(defn- retract-entity-op?
  [item]
  (and (vector? item)
       (contains? #{:db/retractEntity :db.fn/retractEntity} (first item))))

(defn- expand-block-retracts-to-descendants
  [db tx-data]
  (let [explicit-retracts (->> tx-data
                               (keep (fn [item]
                                       (when (and (retract-entity-op? item)
                                                  (block-ref? (second item)))
                                         (some-> (block-entity db (second item)) :db/id))))
                               set)]
    (mapcat
     (fn [item]
       (if (and (retract-entity-op? item)
                (block-ref? (second item))
                (block-entity db (second item)))
         (let [root (block-entity db (second item))]
           (concat (->> (block-descendants root)
                        (remove #(contains? explicit-retracts (:db/id %)))
                        (map (fn [entity]
                               [:db/retractEntity (block-entity-ref entity)])))
                   [item]))
         [item]))
     tx-data)))

(defn- ^:large-vars/cleanup-todo replay-canonical-outliner-op!
  [conn [op args] rebase-db-before]
  (case op
    :save-block
    (let [[block opts] args
          db @conn
          block-uuid (:block/uuid block)
          block-ent (when block-uuid
                      (d/entity db [:block/uuid block-uuid]))
          block-base (dissoc block :db/id :block/order)
          block' (merge block-base
                        (op-construct/rewrite-block-title-with-retracted-refs db block-base))]
      (when (nil? block-ent)
        (invalid-rebase-op! op {:args args
                                :reason :missing-block}))
      (outliner-core/save-block! conn block' opts))

    :insert-blocks
    (let [[blocks target-id opts] args
          db @conn
          [target sibling?] (rebase-resolve-target-and-sibling db rebase-db-before target-id (:sibling? opts))]
      (when-not (and target (seq blocks))
        (invalid-rebase-op! op {:args args}))
      (outliner-core/insert-blocks! conn
                                    (mapv #(op-construct/rewrite-block-title-with-retracted-refs db %) blocks)
                                    target
                                    (assoc opts :sibling? sibling?)))

    :apply-template
    (let [[template-id target-id opts] args
          template-id' (replay-entity-id-value @conn template-id)
          target-id' (replay-entity-id-value @conn target-id)
          [target sibling?] (rebase-resolve-target-and-sibling @conn rebase-db-before target-id' (:sibling? opts))]
      (when-not (and template-id' (d/entity @conn template-id') target)
        (invalid-rebase-op! op {:args args
                                :reason :missing-template-or-target-block}))
      (let [template-uuid (:block/uuid (d/entity @conn template-id'))
            target-uuid (:block/uuid target)]
        (when-not (and (uuid? template-uuid) (uuid? target-uuid))
          (invalid-rebase-op! op {:args args
                                  :reason :missing-template-or-target-uuid}))
        (let [replace-empty-target? (:replace-empty-target? opts)
              template-blocks' (some->> (:template-blocks opts)
                                        (map-indexed
                                         (fn [idx block]
                                           (let [block' (sanitize-template-block @conn rebase-db-before block)
                                                 block'' (if (and replace-empty-target?
                                                                  (zero? idx)
                                                                  (nil? (:block/uuid block')))
                                                           ;; Keep replace-empty-target replay consistent with
                                                           ;; initial apply-template payload where the first
                                                           ;; block uuid is the target uuid.
                                                           (assoc block' :block/uuid target-uuid)
                                                           block')]
                                             (when (:block/uuid block'')
                                               block''))))
                                        (remove nil?)
                                        seq
                                        vec)
              opts' (cond-> (-> opts
                                (assoc :sibling? sibling?)
                                (dissoc :template-blocks))
                      template-blocks'
                      (assoc :template-blocks template-blocks'))]
          (outliner-op/apply-ops!
           conn
           [[:apply-template [template-uuid
                              target-uuid
                              opts']]]
           {:gen-undo-ops? false}))))

    :move-blocks
    (let [[ids target-id opts] args
          ids' (replay-entity-id-coll @conn ids)
          target-id' (replay-entity-id-value @conn target-id)
          blocks (keep #(d/entity @conn %) ids')
          [target sibling?] (rebase-resolve-target-and-sibling @conn rebase-db-before target-id' (:sibling? opts))]
      (when (or (empty? blocks) (nil? target))
        (invalid-rebase-op! op {:args args}))
      (when (seq blocks)
        (outliner-core/move-blocks! conn blocks target (assoc opts :sibling? sibling?))))

    :move-blocks-up-down
    (let [[ids up?] args
          ids' (replay-entity-id-coll @conn ids)
          blocks (keep #(d/entity @conn %) ids')]
      (when (seq blocks)
        (outliner-core/move-blocks-up-down! conn blocks up?)))

    :indent-outdent-blocks
    (let [[ids indent? opts] args
          ids' (replay-entity-id-coll @conn ids)
          blocks (keep #(d/entity @conn %) ids')]
      (when (empty? blocks)
        (invalid-rebase-op! op {:args args}))
      (when (seq blocks)
        (outliner-core/indent-outdent-blocks! conn blocks indent? opts)))

    :delete-blocks
    (let [[ids opts] args
          ids' (replay-entity-id-coll @conn ids)
          blocks (keep #(d/entity @conn %) ids')]
      ;; Keep delete replay idempotent under concurrent edits where blocks may already
      ;; be gone, but still leave a debug breadcrumb for malformed/missing targets.
      (when (empty? blocks)
        (log/debug :undo-redo/drop-delete-blocks-replay
                   {:args args}))
      (when (seq blocks)
        (outliner-core/delete-blocks! conn blocks opts)))

    :create-page
    (let [[title opts] args
          page-uuid (:uuid opts)
          existing-page (or (when (uuid? page-uuid)
                              (d/entity @conn [:block/uuid page-uuid]))
                            (ldb/get-page @conn title))]
      (if (and existing-page
               (not (ldb/recycled? existing-page)))
        [(:block/title existing-page) (:block/uuid existing-page)]
        (outliner-page/create! conn title opts)))

    :delete-page
    (let [[page-uuid opts] args]
      (outliner-page/delete! conn page-uuid opts))

    :upsert-property
    (apply outliner-property/upsert-property! conn args)

    :restore-recycled
    (let [[root-id] args
          root-ref (cond
                     (and (vector? root-id)
                          (= :block/uuid (first root-id)))
                     root-id

                     (uuid? root-id)
                     [:block/uuid root-id]

                     :else
                     root-id)
          root (d/entity @conn root-ref)
          tx-data (when root
                    (seq (outliner-recycle/restore-tx-data @conn root)))]
      (when-not tx-data
        (invalid-rebase-op! op {:args args
                                :reason :invalid-restore-target}))
      (ldb/transact! conn tx-data
                     {:outliner-op :restore-recycled}))

    :recycle-delete-permanently
    (let [[root-id] args
          root-ref (cond
                     (and (vector? root-id)
                          (= :block/uuid (first root-id)))
                     root-id

                     (uuid? root-id)
                     [:block/uuid root-id]

                     :else
                     root-id)
          root (d/entity @conn root-ref)
          tx-data (when root
                    (seq (outliner-recycle/permanently-delete-tx-data @conn root)))]
      ;; Keep replay idempotent under concurrent edits where the recycled root may
      ;; already be permanently removed by a preceding tx.
      (when (seq tx-data)
        (ldb/transact! conn tx-data
                       {:outliner-op :recycle-delete-permanently})))

    (let [[tx-data tx-meta] args
          tx-data (expand-block-retracts-to-descendants @conn tx-data)]
      (when-let [tx-data (seq tx-data)]
        (ldb/transact! conn tx-data tx-meta)))))

(defn ^:large-vars/cleanup-todo apply-history-action!
  "The default worker-undo-redo/*apply-history-action! implementation:
  replays a recorded entry's forward or inverse outliner ops (undo?
  selects which) against the *current* db state, since it may have moved
  on since the entry was recorded."
  [repo tx-id undo? tx-meta]
  (if-let [conn (worker-state/get-datascript-conn repo)]
    (if-let [action (or (local-tx-log/get-local-tx-entry repo tx-id)
                        (inline-history-action tx-meta))]
      (if (= :fix (:outliner-op action))
        {:applied? false
         :reason :unsupported-history-action
         :action action}
        (let [{:keys [tx reversed-tx forward-outliner-ops inverse-outliner-ops]} action
              ops (->> (if undo? inverse-outliner-ops forward-outliner-ops)
                       (filter (fn [op-entry]
                                 (and (sequential? op-entry)
                                      (contains? op-construct/semantic-outliner-ops
                                                 (first op-entry)))))
                       seq)
              tx-data (-> (if undo? reversed-tx tx)
                          normalize-tx-data-for-rebase)
              ops' (if (seq ops)
                     ops
                     [[:transact [tx-data nil]]])
              history-tx-id (let [provided-history-tx-id (:db-sync/tx-id tx-meta)]
                              (if (and (uuid? provided-history-tx-id)
                                       (not= provided-history-tx-id tx-id))
                                provided-history-tx-id
                                (random-uuid)))
              tx-meta' (cond-> {:outliner-op (:outliner-op action)
                                :local-tx? true
                                :gen-undo-ops? false
                                :persist-op? true
                                :undo? undo?
                                :redo? (:redo? tx-meta)
                                :db-sync/tx-id history-tx-id
                                :db-sync/source-tx-id (or (:db-sync/source-tx-id tx-meta)
                                                          tx-id)
                                :db-sync/forward-outliner-ops (if undo?
                                                                (:inverse-outliner-ops action)
                                                                (:forward-outliner-ops action))
                                :db-sync/inverse-outliner-ops (if undo?
                                                                (:forward-outliner-ops action)
                                                                (:inverse-outliner-ops action))})]
          (if (seq ops')
            (try
              (when (seq ops)
                (op-construct/assert-no-numeric-entity-ids! @conn ops :history-action-ops))
              (ldb/batch-transact-with-temp-conn!
               conn
               tx-meta'
               (fn [conn]
                 (doseq [op ops']
                   (replay-canonical-outliner-op! conn op nil))))
              {:applied? true
               :history-tx-id history-tx-id}
              (catch :default e
                (let [reason (history-action-error-reason e)]
                  (when-not (expected-history-action-error-reason? reason)
                    (log/error ::undo-redo-failed e))
                  {:applied? false
                   :reason reason
                   :action action})))
            {:applied? false :reason :unsupported-history-action
             :action action})))
      {:applied? false
       :reason :missing-history-action
       :tx-id tx-id})
    (let [data {:repo repo :op :apply-history-action}]
      (log/error :undo-redo/missing-db data)
      (throw (ex-info "missing-db" data)))))
