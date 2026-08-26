(ns frontend.db.subs-loader
  "Batch renderer snapshot requests across every slot type."
  (:require [frontend.state :as state]
            [promesa.core :as p]))

(def ^:private limits {:blocks 1000 :children 200 :resources 200})
(def ^:private slot-kind {:block :blocks :children :children :resource :resources})

(defonce ^:private *batch (atom {}))

(defonce ^:private *inflight (atom {}))

(defonce ^:private *stats (atom {:dedup-hits 0 :new-entries 0 :flushes 0 :batch-sizes {}}))

(defn get-stats
  "Snapshot of subs-loader batching counters: :dedup-hits counts load! calls
  that reused an already-pending OR already-in-flight slot request (see
  `load!` and `*inflight`) instead of firing a duplicate; :batch-sizes
  tallies entries per slot kind across flushes. Dev-only instrumentation for
  the server-client migration latency baseline (see docs/agent-guide); does
  not affect batching or the `limits` above."
  []
  @*stats)

(defn reset-stats!
  []
  (reset! *stats {:dedup-hits 0 :new-entries 0 :flushes 0 :batch-sizes {}}))

(defn- reject!
  [entries error]
  (doseq [{:keys [result]} entries]
    (p/reject! result error)))

(defn- take-batch!
  []
  (let [entries (vals @*batch)]
    (reset! *batch {})
    entries))

(defn- request-groups
  [entries]
  (let [entries-by-kind (group-by (comp slot-kind first :slot-key) entries)]
    (mapcat (fn [[kind limit]]
              (map vec (partition-all limit (get entries-by-kind kind))))
            limits)))

(defn- worker-request
  [entries]
  (reduce (fn [request {:keys [slot-key]}]
            (update request (slot-kind (first slot-key)) conj (second slot-key)))
          {:blocks [] :children [] :resources []}
          entries))

(defn- entry-response
  [response {:keys [slot-key]}]
  (if-let [group (get-in response [:groups slot-key])]
    {:basis-rev (:basis-rev response)
     :slots (select-keys (:slots response) group)}
    (throw (ex-info "Missing renderer snapshot group"
                    {:slot-key slot-key}))))

(defn- untrack-inflight!
  [entries]
  (swap! *inflight
         (fn [inflight]
           (reduce (fn [m {:keys [graph-id slot-key]}]
                     (dissoc m [graph-id slot-key]))
                   inflight
                   entries))))

(defn- flush!
  []
  (when @state/db-worker-ready?
    (let [batch (take-batch!)]
      ;; Moved into *inflight* (not just dropped after take-batch! clears
      ;; *batch*) so a load! for the same slot arriving while this request is
      ;; still in the air reuses this promise instead of firing a duplicate
      ;; request -- *batch* alone only dedupes calls that land before the
      ;; batch is sent, not across the flush boundary (Fase 6).
      (swap! *inflight into (map (fn [entry] [[(:graph-id entry) (:slot-key entry)] entry]) batch))
      (swap! *stats
             (fn [s]
               (-> s
                   (update :flushes inc)
                   (update :batch-sizes
                           #(merge-with + %
                                        (frequencies (map (comp slot-kind first :slot-key) batch)))))))
      (doseq [[graph-id graph-entries] (group-by :graph-id batch)
              entries (request-groups graph-entries)]
        (-> (state/<invoke-db-worker :thread-api/get-render-snapshots
                                     graph-id
                                     (worker-request entries))
            (p/then (fn [response]
                      (try
                        (let [values (mapv #(entry-response response %) entries)]
                          (doseq [[entry value] (map vector entries values)]
                            (p/resolve! (:result entry) value)))
                        (catch :default error
                          (reject! entries error)))))
            (p/catch #(reject! entries %))
            (p/finally #(untrack-inflight! entries)))))))

(defn- flush-when-db-worker-ready!
  [_key _ref _old-value ready?]
  (when ready?
    (flush!)))

(add-watch state/db-worker-ready?
           ::flush-pending-loads
           flush-when-db-worker-ready!)

(defn load!
  [graph-id slot-key schedule!]
  (let [entry-key [graph-id slot-key]]
    (if-let [result (or (get-in @*batch [entry-key :result])
                        (get-in @*inflight [entry-key :result]))]
      (do (swap! *stats update :dedup-hits inc)
          result)
      (let [result (p/deferred)
            schedule? (empty? @*batch)]
        (swap! *stats update :new-entries inc)
        (swap! *batch assoc entry-key
               {:graph-id graph-id :slot-key slot-key :result result})
        (when schedule?
          (schedule! flush!))
        result))))

(defn reject-pending!
  [error]
  (let [entries (concat (vals @*batch) (vals @*inflight))]
    (reset! *batch {})
    (reset! *inflight {})
    (reject! entries error)))
