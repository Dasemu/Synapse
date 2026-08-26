(ns frontend.worker.handler.semantic
  "Semantic API operations (Fase 9), ported from
   deps/db-sync/src/logseq/db_sync/worker/handler/semantic.cljs. That version
   opened a DataScript conn over a Cloudflare Durable Object's own SQLite and
   broadcast changes to WS clients explicitly; the daemon already has the
   authoritative conn for its bound repo (worker.state/get-datascript-conn)
   and any transact! against it is picked up by the conn listener installed
   in worker.db-listener, which already broadcasts to every connected client
   -- so unlike the CF-Worker version, nothing here calls broadcast
   explicitly. Bodies arrive here already JSON-parsed by the HTTP layer (see
   db_worker_node.cljs's semantic-request branch), so there is no async body
   read either: every operation below is synchronous, matching how DataScript
   itself is synchronous.

   Registers a single :thread-api/semantic-request, dispatched over by
   handler keyword (the same handler keywords semantic-routes/match
   resolves), rather than one def-thread-api per operation -- the CF-Worker
   original used one big `case` in the same way, and callers already go
   through the route table to get the handler keyword."
  (:require [clojure.string :as string]
            [datascript.core :as d]
            [frontend.common.thread-api :refer [def-thread-api]]
            [frontend.worker.platform :as platform]
            [frontend.worker.state :as worker-state]
            [logseq.common.util :as common-util]
            [logseq.common.util.date-time :as date-time-util]
            [logseq.common.util.page-ref :as page-ref]
            [logseq.db :as ldb]
            [logseq.db.frontend.asset :as db-asset]
            [logseq.db.frontend.content :as db-content]
            [logseq.db.frontend.db :as db-db]
            [logseq.db.frontend.entity-util :as entity-util]
            [logseq.db.frontend.property.type :as db-property-type]
            [logseq.outliner.core :as outliner-core]
            [logseq.outliner.page :as outliner-page]
            [logseq.outliner.property :as outliner-property]
            [logseq.outliner.tree :as otree]))

(def ^:private default-limit 50)
(def ^:private max-limit 200)
(def ^:private max-asset-size (* 100 1024 1024))

(defn- api-error
  ([status code message] (api-error status code message nil))
  ([status code message data]
   (ex-info message (merge {:status status :code code} data))))

(defn- bad-request [message] (throw (api-error 400 :bad-request message)))
(defn- not-found [] (throw (api-error 404 :not-found "not found")))
(defn- api-error! [message status] (throw (api-error status :semantic-error message)))

(defn- uuid-string [value]
  (when value (str value)))

(defn- page? [entity]
  (or (ldb/internal-page? entity) (ldb/journal? entity)))

(defn- asset? [entity]
  (string? (:logseq.property.asset/type entity)))

(defn- entity-ident [entity]
  (when-let [ident (:db/ident entity)]
    (str (namespace ident) "/" (name ident))))

(defn- tag? [entity]
  (ldb/class? entity))

(defn- property? [entity]
  (ldb/property? entity))

(defn- block-kind [entity]
  (cond
    (asset? entity) "asset"
    (property? entity) "property"
    (tag? entity) "tag"
    (page? entity) "page"
    :else "block"))

(defn- block-response [block]
  (cond-> {:uuid (uuid-string (:block/uuid block))
           :kind (block-kind block)
           :title (:block/title block)
           :order (:block/order block)}
    (:block/parent block) (assoc :parent-id (uuid-string (:block/uuid (:block/parent block))))
    (:block/page block) (assoc :page-id (uuid-string (:block/uuid (:block/page block))))
    (seq (:block/children block)) (assoc :children (mapv block-response (:block/children block)))))

(defn- tag-response [entity]
  {:uuid (uuid-string (:block/uuid entity))
   :ident (entity-ident entity)
   :title (:block/title entity)})

(defn- property-choice-response [entity]
  {:uuid (uuid-string (:block/uuid entity))
   :ident (entity-ident entity)
   :title (:block/title entity)
   :value (:logseq.property/value entity)})

(defn- property-response [entity]
  (cond-> {:uuid (uuid-string (:block/uuid entity))
           :ident (entity-ident entity)
           :title (:block/title entity)
           :type (some-> (:logseq.property/type entity) name)
           :cardinality (some-> (:db/cardinality entity) name)}
    (seq (:block/_closed-value-property entity))
    (assoc :choices (mapv property-choice-response (:block/_closed-value-property entity)))))

(defn- asset-response [entity]
  {:uuid (uuid-string (:block/uuid entity))
   :title (:block/title entity)
   :type (:logseq.property.asset/type entity)
   :size (:logseq.property.asset/size entity)
   :checksum (:logseq.property.asset/checksum entity)})

(defn- asset-file-name [entity]
  (str (:block/uuid entity) "." (:logseq.property.asset/type entity)))

(defn- entities-by-avet [db attribute & [value]]
  (->> (if (some? value)
         (d/datoms db :avet attribute value)
         (d/datoms db :avet attribute))
       (map #(d/entity db (:e %)))
       (remove entity-util/hidden?)))

(defn- parse-limit [{:keys [limit]}]
  (let [parsed (when limit (js/parseInt limit 10))]
    (cond
      (nil? limit) default-limit
      (or (js/isNaN parsed) (not (pos? parsed))) nil
      :else (min parsed max-limit))))

(def ^:private text-encoder (js/TextEncoder.))
(def ^:private text-decoder (js/TextDecoder.))

(defn- encode-cursor [key]
  (let [payload (.encode text-encoder (js/JSON.stringify (clj->js key)))
        binary (.join (.map (js/Array.from payload) #(js/String.fromCharCode %)) "")]
    (js/btoa binary)))

(defn- decode-cursor [value]
  (when value
    (try
      (let [binary (js/atob value)
            payload (js/Uint8Array.from (js/Array.from binary #(.charCodeAt % 0)))
            decoded (js->clj (js/JSON.parse (.decode text-decoder payload)))]
        (when (and (vector? decoded) (= 2 (count decoded)) (every? string? decoded)) decoded))
      (catch :default _ nil))))

(defn- sort-key [item]
  [(or (:order item) (:block/order item)
       (string/lower-case (or (:title item) (:block/title item) "")))
   (str (or (:uuid item) (:block/uuid item) ""))])

(defn- paginate [items limit cursor]
  (let [sorted (sort-by sort-key items)
        remaining (if cursor (drop-while #(not (pos? (compare (sort-key %) cursor))) sorted) sorted)
        selected (vec (take (inc limit) remaining))
        more? (> (count selected) limit)
        page (vec (take limit selected))]
    (cond-> {:items page}
      more? (assoc :next-cursor (encode-cursor (sort-key (last page)))))))

(defn- paginate-bounded [items limit cursor]
  (let [selected (reduce (fn [selected item]
                           (if (or (nil? cursor)
                                   (pos? (compare (sort-key item) cursor)))
                             (->> (conj selected item)
                                  (sort-by sort-key)
                                  (take (inc limit))
                                  vec)
                             selected))
                         []
                         items)
        more? (> (count selected) limit)
        page (vec (take limit selected))]
    (cond-> {:items page}
      more? (assoc :next-cursor (encode-cursor (sort-key (last page)))))))

(defn- pagination-options [query-params]
  {:limit (parse-limit query-params)
   :raw-cursor (:cursor query-params)
   :cursor (decode-cursor (:cursor query-params))})

(defn- parse-timestamp [raw]
  (cond
    (nil? raw) nil
    (not (re-matches #"\d+" raw)) ::invalid
    :else (js/parseInt raw 10)))

(defn- time-filter-options [query-params]
  (let [created-after (parse-timestamp (:created-after query-params))
        updated-after (parse-timestamp (:updated-after query-params))]
    {:created-after created-after
     :updated-after updated-after
     :invalid? (or (= ::invalid created-after) (= ::invalid updated-after))}))

(defn- after-time-filters? [entity {:keys [created-after updated-after]}]
  (and (or (nil? created-after)
           (and (number? (:block/created-at entity))
                (> (:block/created-at entity) created-after)))
       (or (nil? updated-after)
           (and (number? (:block/updated-at entity))
                (> (:block/updated-at entity) updated-after)))))

(defn- paginated-response
  ([query-params response-key items]
   (paginated-response query-params response-key items identity))
  ([query-params response-key items response-fn]
   (paginated-response query-params response-key items response-fn paginate))
  ([query-params response-key items response-fn paginate-fn]
   (let [{:keys [limit raw-cursor cursor]} (pagination-options query-params)
         time-filters (time-filter-options query-params)]
     (cond
       (nil? limit) (bad-request "invalid limit")
       (and raw-cursor (nil? cursor)) (bad-request "invalid cursor")
       (:invalid? time-filters) (bad-request "invalid time filter")
       :else (let [items (filter #(after-time-filters? % time-filters) items)
                   {:keys [items next-cursor]} (paginate-fn items limit cursor)]
               (cond-> {response-key (mapv response-fn items)}
                 next-cursor (assoc :next-cursor next-cursor)))))))

(defn- indexed-reference-response
  ([db query-params response-key attribute target-id]
   (indexed-reference-response db query-params response-key attribute target-id {}))
  ([db query-params response-key attribute target-id {:keys [entity-filter response-fn]
                                                        :or {entity-filter (constantly true)
                                                             response-fn block-response}}]
   (let [{:keys [limit raw-cursor cursor]} (pagination-options query-params)
         time-filters (time-filter-options query-params)
         cursor-e (when (= "entity-id" (first cursor))
                    (js/parseInt (second cursor) 10))]
     (cond
       (nil? limit) (bad-request "invalid limit")
       (and raw-cursor (or (nil? cursor) (js/isNaN cursor-e))) (bad-request "invalid cursor")
       (:invalid? time-filters) (bad-request "invalid time filter")
       :else
       (let [datoms (cond->> (d/datoms db :avet attribute target-id)
                      cursor-e (drop-while #(<= (:e %) cursor-e))
                      true (remove #(entity-util/hidden? (d/entity db (:e %))))
                      true (filter #(after-time-filters? (d/entity db (:e %)) time-filters))
                      true (filter #(entity-filter (d/entity db (:e %)))))
             selected (vec (take (inc limit) datoms))
             more? (> (count selected) limit)
             page (take limit selected)]
         (cond-> {response-key (mapv #(response-fn (d/entity db (:e %))) page)}
           more? (assoc :next-cursor
                        (encode-cursor ["entity-id" (str (:e (last page)))]))))))))

(defn- find-entity [db id]
  (when-let [parsed (try (uuid id) (catch :default _ nil))]
    (d/entity db [:block/uuid parsed])))

(defn- find-visible-entity [db id]
  (when-let [entity (find-entity db id)]
    (when-not (entity-util/hidden? entity) entity)))

(def ^:private page-block-pull-keys
  [:db/id :block/uuid :block/title :block/order
   :logseq.property/hide? :logseq.property/deleted-at
   {:block/parent [:db/id :block/uuid]}
   {:block/page [:db/id :block/uuid]}])

(defn- page-root-blocks [db page]
  (let [page-id (:db/id page)]
    (->> (d/datoms db :avet :block/page page-id)
         (filter #(seq (d/datoms db :eavt (:e %) :block/parent page-id)))
         (map #(:e %))
         (map #(d/pull db [:db/id :block/uuid :block/title :block/order
                         :block/created-at :block/updated-at
                         :logseq.property/hide? :logseq.property/deleted-at
                         {:block/parent [:db/id :block/uuid]}
                         {:block/page [:db/id :block/uuid]}]
                    %))
         (remove entity-util/hidden?))))

(defn- page-block-tree [db root]
  (let [blocks (->> (ldb/get-block-and-children db (:block/uuid root))
                    (remove #(entity-util/hidden? (d/entity db (:db/id %)))))]
    (some->> blocks
             (map #(d/pull db page-block-pull-keys (:db/id %)))
             (otree/non-consecutive-blocks->vec-tree)
             first
             block-response)))

(defn- paginated-page-blocks-response [db page query-params]
  (let [{:keys [limit raw-cursor cursor]} (pagination-options query-params)
        time-filters (time-filter-options query-params)]
    (cond
      (nil? limit) (bad-request "invalid limit")
      (and raw-cursor (nil? cursor)) (bad-request "invalid cursor")
      (:invalid? time-filters) (bad-request "invalid time filter")
      :else
      (let [{roots :items next-cursor :next-cursor}
            (paginate (filter #(after-time-filters? % time-filters)
                              (page-root-blocks db page))
                      limit
                      cursor)]
        (cond-> {:blocks (mapv #(page-block-tree db %) roots)}
          next-cursor (assoc :next-cursor next-cursor))))))

(defn- list-pages [db]
  (->> [:logseq.class/Page :logseq.class/Journal]
       (keep #(d/entid db %))
       (mapcat #(entities-by-avet db :block/tags %))))

(defn- page-ref-values [title]
  (when (string? title)
    (->> (re-seq page-ref/page-ref-re title)
         (map (comp string/trim second))
         (remove string/blank?)
         distinct)))

(defn- integer-string? [value]
  (boolean (re-matches #"-?\d+" value)))

(defn- resolve-page-ref! [conn value]
  (cond
    (common-util/uuid-string? value)
    (when-let [entity (d/entity @conn [:block/uuid (uuid value)])]
      {:block/uuid (:block/uuid entity)
       :block/title value})

    (integer-string? value)
    (when-let [entity (d/entity @conn (js/parseInt value 10))]
      {:block/uuid (:block/uuid entity)
       :block/title value})

    :else
    (let [page (or (ldb/get-page @conn value)
                   (let [[_ page-id] (outliner-page/create! conn value {})]
                     (d/entity @conn [:block/uuid page-id])))]
      {:block/uuid (:block/uuid page)
       :block/title (:block/title page)})))

(defn- prepare-block-title! [conn title]
  (let [refs (->> (page-ref-values title)
                  (keep #(resolve-page-ref! conn %))
                  (reduce (fn [result ref]
                            (assoc result (:block/uuid ref) ref)) {})
                  vals
                  vec)]
    {:block/title (db-content/title-ref->id-ref title refs :replace-tag? false)
     :block/refs (mapv #(vector :block/uuid (:block/uuid %)) refs)}))

(defn- tree-block [conn node]
  (assoc (prepare-block-title! conn (:title node)) :block/uuid (random-uuid)))

(defn- insert-tree! [conn target nodes position]
  (let [nodes (vec nodes)
        blocks (mapv #(tree-block conn %) nodes)]
    (when (seq blocks)
      (outliner-core/insert-blocks! conn blocks target
                                    {:sibling? false :top? (= position "prepend")
                                     :bottom? (not= position "prepend") :keep-uuid? true})
      (mapv (fn [node block]
              (let [children (when (seq (:children node))
                               (insert-tree! conn (d/entity @conn [:block/uuid (:block/uuid block)])
                                             (:children node) "append"))]
                (cond-> {:uuid (str (:block/uuid block)) :title (:title node)}
                  (seq children) (assoc :children children))))
            nodes blocks))))

(defn- today-journal-day []
  (let [now (js/Date.)]
    (+ (* (.getFullYear now) 10000) (* (inc (.getMonth now)) 100) (.getDate now))))

(defn- ensure-today-page! [conn]
  (let [day (today-journal-day)]
    (or (ldb/get-journal-page-by-day @conn day)
        (let [formatter (:logseq.property.journal/title-format (d/entity @conn :logseq.class/Journal))
              title (date-time-util/int->journal-title day formatter)
              [_ page-id] (outliner-page/create! conn title {:journal? true :today-journal? true})]
          (d/entity @conn [:block/uuid page-id])))))

(defn- resolve-property-ident [db value]
  (or (when-let [entity (find-entity db value)] (:db/ident entity))
      (when (string? value)
        (let [ident (keyword value)]
          (or (when (d/entity db ident) ident)
              (some->> (d/datoms db :avet :block/title value)
                       (map #(d/entity db (:e %)))
                       (filter property?)
                       first
                       :db/ident))))))

(defn- resolve-property [db value]
  (when-let [ident (resolve-property-ident db value)]
    (let [property (d/entity db ident)]
      (when-not (entity-util/hidden? property)
        property))))

(def ^:private allowed-property-types
  (into (set db-property-type/user-built-in-property-types)
        db-property-type/user-allowed-internal-property-types))

(def ^:private allowed-cardinalities
  #{:db.cardinality/one :db.cardinality/many})

(defn- valid-property-schema? [{:keys [type cardinality]}]
  (and (or (nil? type)
           (and (string? type) (contains? allowed-property-types (keyword type))))
       (or (nil? cardinality)
           (and (string? cardinality) (contains? allowed-cardinalities (keyword cardinality))))))

(def ^:private task-status-idents
  {"todo" :logseq.property/status.todo
   "doing" :logseq.property/status.doing
   "in-review" :logseq.property/status.in-review
   "done" :logseq.property/status.done
   "canceled" :logseq.property/status.canceled
   "backlog" :logseq.property/status.backlog})

(def ^:private task-priority-idents
  {"low" :logseq.property/priority.low
   "medium" :logseq.property/priority.medium
   "high" :logseq.property/priority.high
   "urgent" :logseq.property/priority.urgent})

(def ^:private property-choice-aliases
  {:logseq.property/status task-status-idents
   :logseq.property/priority task-priority-idents})

(defn- resolve-ref-entity [db property-ident value]
  (let [value (if (string? value)
                (get-in property-choice-aliases
                        [property-ident (string/lower-case value)] value)
                value)
        entity (cond
                 (number? value) (d/entity db value)
                 (qualified-keyword? value) (d/entity db value)
                 (and (string? value) (common-util/uuid-string? value))
                 (d/entity db [:block/uuid (uuid value)])
                 (string? value)
                 (or (d/entity db (keyword value))
                     (some->> (d/datoms db :avet :block/title value)
                              (map #(d/entity db (:e %)))
                              (filter #(if (= property-ident :block/tags)
                                         (entity-util/class? %)
                                         true))
                              first))
                 :else nil)]
    (when (and entity
               (or (not= property-ident :block/tags) (entity-util/class? entity)))
      (:db/id entity))))

(defn- prepare-property-value [db property-ident value]
  (let [property (d/entity db property-ident)]
    (if-not (= :db.type/ref (:db/valueType property))
      {:value value}
      (let [many-input? (or (sequential? value) (set? value))
            values (if many-input? value [value])
            resolved (mapv #(resolve-ref-entity db property-ident %) values)]
        (if (every? some? resolved)
          {:value (if many-input? resolved (first resolved))}
          {:error (if (= property-ident :block/tags)
                    "property value must resolve to an existing class by UUID, ident, or title"
                    "property value must resolve to an existing entity by UUID, ident, or title")})))))

(defn- set-prepared-property! [conn block-id property-ident value reset-existing-values?]
  (if (coll? value)
    (do
      (when reset-existing-values?
        (outliner-property/remove-block-property! conn block-id property-ident))
      (doseq [item value]
        (outliner-property/batch-set-property! conn [block-id] property-ident item {:entity-id? true})))
    (outliner-property/set-block-property! conn block-id property-ident value)))

(defn- task-response [task]
  (cond-> (block-response task)
    (:logseq.property/status task)
    (assoc :status (property-choice-response (:logseq.property/status task)))
    (:logseq.property/priority task)
    (assoc :priority (property-choice-response (:logseq.property/priority task)))
    (:logseq.property/scheduled task) (assoc :scheduled (:logseq.property/scheduled task))
    (:logseq.property/deadline task) (assoc :deadline (:logseq.property/deadline task))))

(defn- resolve-property-choice [db property-ident aliases selector]
  (let [selector (get aliases selector selector)
        choice-id (resolve-ref-entity db property-ident selector)
        choice (when choice-id (d/entity db choice-id))
        property-id (d/entid db property-ident)]
    (when (and choice-id
               (some #(= property-id (:v %))
                     (d/datoms db :eavt choice-id :block/closed-value-property)))
      choice)))

(defn- search-result-response [entity]
  (let [kind (block-kind entity)
        type (case kind "tag" "tags" "property" "properties" "asset" "assets" "blocks")
        response (case kind
                   "tag" (tag-response entity)
                   "property" (property-response entity)
                   "asset" (asset-response entity)
                   (block-response entity))]
    (assoc response :resource type)))

(defn- search-results [db query types]
  (let [needle (string/lower-case query)
        enabled (if (seq types) (set (string/split types #",")) #{"blocks" "tags" "properties" "assets"})
        title-matches (keep (fn [datom]
                              (try
                                (when (string/includes? (string/lower-case (:v datom)) needle)
                                  (:e datom))
                                (catch :default _
                                  nil)))
                            (d/datoms db :avet :block/title))]
    (->> title-matches
         (map #(d/entity db %))
         (remove entity-util/hidden?)
         (filter (fn [entity]
                   (let [kind (block-kind entity)
                         type (case kind "tag" "tags" "property" "properties" "asset" "assets" "blocks")]
                     (and (contains? enabled type)
                          (string/includes? (string/lower-case (or (:block/title entity) "")) needle))))))))

(defn- handle-pages [{:keys [conn db handler path-params query-params body]}]
  (case handler
    :semantic/pages-list
    (paginated-response query-params :blocks (list-pages db) block-response)

    :semantic/pages-create
    (if-not (seq (:title body))
      (bad-request "missing title")
      (let [[title page-id] (outliner-page/create! conn (:title body) {})]
        {:uuid (str page-id) :kind "page" :title title}))

    :semantic/pages-blocks
    (if-let [page (find-visible-entity db (:page-id path-params))]
      (if-not (page? page)
        (bad-request "block is not a page")
        (paginated-page-blocks-response db page query-params))
      (not-found))

    :semantic/pages-references
    (if-let [page (find-visible-entity db (:page-id path-params))]
      (if (page? page)
        (indexed-reference-response db query-params :references :block/refs (:db/id page))
        (not-found))
      (not-found))

    :semantic/pages-get
    (if-let [page (find-visible-entity db (:page-id path-params))]
      (if (page? page) (block-response page) (not-found))
      (not-found))

    :semantic/pages-update
    (let [page (find-entity db (:page-id path-params))]
      (if (or (not (page? page)) (not (seq (:title body))))
        (bad-request "invalid page-id or title")
        (do
          (outliner-core/save-block! conn {:block/uuid (:block/uuid page) :block/title (:title body)})
          (assoc (block-response page) :title (:title body)))))

    :semantic/pages-delete
    (if-let [page (find-entity db (:page-id path-params))]
      (if-not (page? page)
        (bad-request "block is not a page")
        (do (outliner-page/delete! conn (:block/uuid page)) nil))
      (not-found))))

(defn- handle-blocks [{:keys [conn db handler path-params body]}]
  (case handler

    :semantic/blocks-get
    (if-let [block (find-visible-entity db (:block-id path-params))]
      (block-response block)
      (not-found))

    :semantic/blocks-update
    (let [block (find-entity db (:block-id path-params))]
      (if (or (nil? block) (not (seq (:title body))))
        (bad-request "invalid block-id or title")
        (do
          (outliner-core/save-block! conn
                                     (assoc (prepare-block-title! conn (:title body))
                                            :block/uuid (:block/uuid block)))
          {:uuid (str (:block/uuid block)) :kind (block-kind block) :title (:title body)})))

    :semantic/blocks-delete
    (if-let [block (find-entity db (:block-id path-params))]
      (do
        (if (page? block)
          (outliner-page/delete! conn (:block/uuid block))
          (outliner-core/delete-blocks! conn [block] {}))
        nil)
      (not-found))

    :semantic/blocks-move
    (let [block-ids (:block-ids body)
          blocks (mapv #(find-entity db %) block-ids)
          target (find-entity db (:target-id body))]
      (if (or (not (seq block-ids))
              (some nil? blocks) (nil? target)
              (not= (count blocks) (count (distinct (map :db/id blocks))))
              (some #(= (:db/id target) (:db/id %)) blocks)
              (not (contains? #{"before" "after" "first-child" "last-child"} (:position body))))
        (bad-request "invalid blocks, target, or position")
        (let [position (:position body)
              opts (case position
                     "before" {:sibling? true :top? true}
                     "after" {:sibling? true :bottom? true}
                     "first-child" {:sibling? false :top? true}
                     "last-child" {:sibling? false :bottom? true})]
          (outliner-core/move-blocks! conn blocks target opts)
          {:uuids (mapv #(str (:block/uuid %)) blocks) :moved true})))

    :semantic/blocks-insert-children
    (let [target (find-entity db (:block-id path-params))]
      (if (or (nil? target) (not (contains? #{"append" "prepend"} (:position body)))
              (not (seq (:blocks body))))
        (bad-request "invalid target, position, or blocks")
        {:blocks (insert-tree! conn target (:blocks body) (:position body))}))

    :semantic/blocks-insert-tree
    (let [target (find-entity db (:target-id body))]
      (if (or (nil? target) (not (seq (:blocks body))))
        (bad-request "invalid target or blocks")
        {:blocks (insert-tree! conn target (:blocks body) (or (:position body) "append"))}))))

(defn- handle-block-properties [{:keys [conn db handler path-params body]}]
  (case handler

    :semantic/blocks-set-property
    (let [block (find-entity db (:block-id path-params))
          property-ident (resolve-property-ident db (:property-id path-params))
          prepared (when property-ident (prepare-property-value db property-ident (:value body)))]
      (cond
        (nil? block) (bad-request "block not found")
        (nil? property-ident) (bad-request "property not found")
        (not (contains? body :value)) (bad-request "missing property value")
        (:error prepared) (bad-request (:error prepared))
        :else
        (do (set-prepared-property! conn (:db/id block) property-ident (:value prepared) true)
            {:updated true})))

    :semantic/blocks-delete-property
    (let [block (find-entity db (:block-id path-params))
          property-ident (resolve-property-ident db (:property-id path-params))]
      (if (or (nil? block) (nil? property-ident))
        (bad-request "invalid block or property")
        (do
          (outliner-property/remove-block-property! conn (:db/id block) property-ident)
          nil)))

    :semantic/blocks-batch-set-property
    (let [entries (:entries body)
          resolved (mapv (fn [{:keys [block-id property-id value]}]
                           (let [property-ident (resolve-property-ident db property-id)]
                             {:block (find-entity db block-id)
                              :property-ident property-ident
                              :prepared (when property-ident
                                          (prepare-property-value db property-ident value))})) entries)]
      (if (or (not (seq entries))
              (some #(or (nil? (:block %)) (nil? (:property-ident %))
                         (:error (:prepared %))) resolved))
        (bad-request "invalid batch property entry")
        (do (ldb/batch-transact-with-temp-conn!
             conn {:outliner-op :batch-set-property}
             (fn [temp-conn]
               (doseq [{:keys [block property-ident prepared]} resolved]
                 (set-prepared-property! temp-conn (:db/id block) property-ident (:value prepared)
                                         (true? (:isResetExistingValues body))))))
            {:updated (count resolved)})))

    :semantic/blocks-batch-delete-property
    (let [entries (:entries body)
          resolved (mapv (fn [{:keys [block-id property-id]}]
                           {:block (find-entity db block-id)
                            :property-ident (resolve-property-ident db property-id)}) entries)]
      (if (or (not (seq entries)) (some #(or (nil? (:block %)) (nil? (:property-ident %))) resolved))
        (bad-request "invalid batch property entry")
        (do
          (ldb/batch-transact-with-temp-conn!
           conn {:outliner-op :batch-remove-property}
           (fn [temp-conn]
             (doseq [{:keys [block property-ident]} resolved]
               (outliner-property/remove-block-property! temp-conn (:db/id block) property-ident))))
          {:deleted (count resolved)})))))

(defn- handle-capture-and-tags [{:keys [conn db handler path-params query-params body]}]
  (case handler

    :semantic/capture
    (if-not (seq (:blocks body))
      (bad-request "missing blocks")
      (let [today (ensure-today-page! conn)
            inserted (insert-tree! conn today (:blocks body) "append")]
        {:page-id (str (:block/uuid today)) :blocks inserted}))

    :semantic/tags-list
    (paginated-response query-params :tags (entities-by-avet db :block/tags :logseq.class/Tag) tag-response)

    :semantic/tags-create
    (if-not (seq (:title body))
      (bad-request "missing title")
      (let [[title tag-id] (outliner-page/create! conn (:title body) {:class? true})]
        {:uuid (str tag-id) :title title}))

    :semantic/tags-get
    (if-let [tag (find-visible-entity db (:tag-id path-params))]
      (if (tag? tag) (tag-response tag) (not-found))
      (not-found))

    :semantic/tags-objects
    (if-let [tag (find-visible-entity db (:tag-id path-params))]
      (if (tag? tag)
        (indexed-reference-response db query-params :objects :block/tags (:db/id tag))
        (not-found))
      (not-found))

    :semantic/tags-update
    (let [tag (find-entity db (:tag-id path-params))]
      (if (or (not (tag? tag)) (not (seq (:title body))))
        (bad-request "invalid tag-id or title")
        (do
          (outliner-core/save-block! conn {:block/uuid (:block/uuid tag) :block/title (:title body)})
          (assoc (tag-response tag) :title (:title body)))))

    :semantic/tags-delete
    (if-let [tag (find-entity db (:tag-id path-params))]
      (if-not (tag? tag)
        (not-found)
        (do (outliner-page/delete! conn (:block/uuid tag)) nil))
      (not-found))))

(defn- handle-tasks [{:keys [conn db handler query-params body]}]
  (case handler
    :semantic/tasks-list
    (let [status (:status query-params)
          priority (:priority query-params)
          content (some-> (:content query-params) string/lower-case)
          status-choice (when status (resolve-property-choice db :logseq.property/status
                                                              task-status-idents status))
          priority-choice (when priority (resolve-property-choice db :logseq.property/priority
                                                                  task-priority-idents priority))]
      (cond
        (and status (nil? status-choice)) (bad-request "invalid task status")
        (and priority (nil? priority-choice)) (bad-request "invalid task priority")
        :else
        (indexed-reference-response
         db query-params :tasks :block/tags :logseq.class/Task
         {:entity-filter
          (fn [entity]
            (and (or (nil? status-choice)
                     (= (:db/id status-choice) (:db/id (:logseq.property/status entity))))
                 (or (nil? priority-choice)
                     (= (:db/id priority-choice) (:db/id (:logseq.property/priority entity))))
                 (or (nil? content)
                     (string/includes? (string/lower-case (or (:block/title entity) "")) content))))
          :response-fn task-response})))

    :semantic/tasks-create
    (let [status (or (:status body) "todo")
          priority (:priority body)
          status-choice (resolve-property-choice db :logseq.property/status task-status-idents status)
          priority-choice (when priority
                            (resolve-property-choice db :logseq.property/priority
                                                     task-priority-idents priority))
          target (when-let [page-id (:page-id body)]
                   (find-entity db page-id))]
      (cond
        (not (seq (:title body))) (bad-request "missing task title")
        (nil? status-choice) (bad-request "invalid task status")
        (and priority (nil? priority-choice)) (bad-request "invalid task priority")
        (and (:page-id body) (not (page? target))) (bad-request "invalid task page-id")
        :else
        (let [task-id (atom nil)]
          (ldb/batch-transact-with-temp-conn!
           conn {:outliner-op :create-task}
           (fn [temp-conn]
             (let [target (if target
                            (d/entity @temp-conn [:block/uuid (:block/uuid target)])
                            (ensure-today-page! temp-conn))
                   inserted (first (insert-tree! temp-conn target [{:title (:title body)}] "append"))
                   task-uuid (uuid (:uuid inserted))
                   task (d/entity @temp-conn [:block/uuid task-uuid])]
               (reset! task-id task-uuid)
               (outliner-property/set-block-property! temp-conn (:db/id task) :block/tags
                                                      (:db/id (d/entity @temp-conn :logseq.class/Task)))
               (outliner-property/set-block-property! temp-conn (:db/id task) :logseq.property/status
                                                      (:db/id status-choice))
               (when priority-choice
                 (outliner-property/set-block-property! temp-conn (:db/id task) :logseq.property/priority
                                                        (:db/id priority-choice)))
               (doseq [property-ident [:logseq.property/scheduled :logseq.property/deadline]
                       :let [value (get body (keyword (name property-ident)))]
                       :when (some? value)]
                 (outliner-property/set-block-property! temp-conn (:db/id task) property-ident value)))))
          (task-response (d/entity @conn [:block/uuid @task-id])))))))

(defn- handle-assets
  [{:keys [repo conn db handler path-params query-params body]}]
  (case handler
    :semantic/assets-list
    (paginated-response query-params :assets
                        (entities-by-avet db :logseq.property.asset/type)
                        asset-response)

    :semantic/assets-create
    (let [{:keys [file-name size checksum encoding page-id title]} query-params
          size (when (and size (re-matches #"\d+" (str size))) (js/parseInt size 10))
          checksum (some-> checksum string/lower-case)
          asset-type (when (string? file-name) (db-asset/asset-path->type file-name))
          target (when page-id (find-entity db page-id))]
      (cond
        (or (not (seq file-name)) (not (seq asset-type))) (bad-request "invalid asset file-name")
        (nil? size) (bad-request "invalid asset size")
        (> size max-asset-size) (api-error! "asset too large" 413)
        (not (and (string? checksum) (re-matches #"[0-9a-f]{64}" checksum)))
        (bad-request "invalid asset checksum")
        (and encoding (not= "base64" encoding)) (bad-request "invalid asset encoding")
        (and page-id (not (page? target))) (bad-request "invalid asset page-id")
        (seq (d/datoms db :avet :logseq.property.asset/checksum checksum))
        (api-error! "asset checksum already exists" 409)
        :else
        (let [asset-id (random-uuid)
              file-name* (str asset-id "." asset-type)
              title (or title (db-asset/asset-name->title file-name))
              payload (if (= "base64" encoding)
                        (js/Buffer.from (str body) "base64")
                        body)]
          (platform/asset-write-bytes! (platform/current) repo file-name* payload)
          (let [target (or target (ensure-today-page! conn))
                block {:block/uuid asset-id
                       :block/title title
                       :block/tags #{:logseq.class/Asset}
                       :logseq.property.asset/type asset-type
                       :logseq.property.asset/size size
                       :logseq.property.asset/checksum checksum}]
            (outliner-core/insert-blocks! conn [block] target
                                          {:sibling? false :bottom? true :keep-uuid? true})
            (asset-response (d/entity @conn [:block/uuid asset-id]))))))

    :semantic/assets-get
    (if-let [asset (find-visible-entity db (:asset-block-id path-params))]
      (if-not (asset? asset)
        (bad-request "block is not an asset")
        (assoc (asset-response asset)
               :url (str "/v1/assets/" (js/encodeURIComponent repo) "/"
                         (js/encodeURIComponent (asset-file-name asset)))))
      (not-found))))

(defn- handle-properties-and-search
  [{:keys [conn db handler path-params query-params body]}]
  (case handler

    :semantic/properties-list
    (paginated-response query-params :properties (->> (db-db/get-all-properties db)
                                                       (remove entity-util/hidden?))
                        property-response)

    :semantic/properties-create
    (if (or (not (seq (:title body))) (not (valid-property-schema? body)))
      (bad-request "invalid property schema")
      (let [property (outliner-property/upsert-property!
                      conn nil
                      {:logseq.property/type (keyword (or (:type body) "default"))
                       :db/cardinality (keyword (or (:cardinality body) "db.cardinality/one"))}
                      {:property-name (:title body)})]
        (property-response property)))

    :semantic/properties-get
    (if-let [property (resolve-property db (:property-id path-params))]
      (if (property? property) (property-response property) (not-found))
      (not-found))

    :semantic/properties-update
    (let [property (resolve-property db (:property-id path-params))]
      (if (or (not (property? property)) (empty? body) (not (valid-property-schema? body)))
        (bad-request "invalid property-id or update")
        (let [schema (cond-> {}
                       (:type body) (assoc :logseq.property/type (keyword (:type body)))
                       (:cardinality body) (assoc :db/cardinality (keyword (:cardinality body))))
              updated (outliner-property/upsert-property!
                       conn (:db/ident property) schema
                       (cond-> {} (:title body) (assoc :property-name (:title body))))]
          (property-response updated))))

    :semantic/properties-delete
    (if-let [property (resolve-property db (:property-id path-params))]
      (if-not (property? property)
        (not-found)
        (if (outliner-page/delete! conn (:block/uuid property))
          nil
          (bad-request "property cannot be deleted")))
      (not-found))

    :semantic/search
    (let [query (:q query-params)]
      (if-not (seq query)
        (bad-request "missing q")
        (paginated-response query-params :results
                            (search-results db query (:types query-params))
                            search-result-response
                            paginate-bounded)))))

(def ^:private page-handlers
  #{:semantic/pages-list :semantic/pages-create :semantic/pages-blocks :semantic/pages-references :semantic/pages-get
    :semantic/pages-update :semantic/pages-delete})

(def ^:private block-handlers
  #{:semantic/blocks-get :semantic/blocks-update :semantic/blocks-delete :semantic/blocks-move
    :semantic/blocks-insert-children :semantic/blocks-insert-tree})

(def ^:private block-property-handlers
  #{:semantic/blocks-set-property :semantic/blocks-delete-property
    :semantic/blocks-batch-set-property :semantic/blocks-batch-delete-property})

(def ^:private capture-and-tag-handlers
  #{:semantic/capture :semantic/tags-list :semantic/tags-create :semantic/tags-get :semantic/tags-objects
    :semantic/tags-update :semantic/tags-delete})

(def ^:private task-handlers
  #{:semantic/tasks-list :semantic/tasks-create})

(def ^:private asset-handlers
  #{:semantic/assets-list :semantic/assets-create :semantic/assets-get})

(def ^:private property-and-search-handlers
  #{:semantic/properties-list :semantic/properties-create :semantic/properties-get
    :semantic/properties-update :semantic/properties-delete
    :semantic/search})

(defn- dispatch [{:keys [handler] :as context}]
  (cond
    (contains? page-handlers handler) (handle-pages context)
    (contains? block-handlers handler) (handle-blocks context)
    (contains? block-property-handlers handler) (handle-block-properties context)
    (contains? capture-and-tag-handlers handler) (handle-capture-and-tags context)
    (contains? task-handlers handler) (handle-tasks context)
    (contains? asset-handlers handler) (handle-assets context)
    (contains? property-and-search-handlers handler) (handle-properties-and-search context)
    :else (not-found)))

(def-thread-api :thread-api/semantic-request
  [repo handler path-params query-params body]
  (if-let [conn (worker-state/get-datascript-conn repo)]
    (dispatch {:repo repo
               :conn conn
               :db @conn
               :handler handler
               :path-params (or path-params {})
               :query-params (or query-params {})
               :body body})
    (throw (api-error 404 :repo-not-found "repo not found"))))
