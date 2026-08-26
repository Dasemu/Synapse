(ns frontend.worker.db-worker-test
  (:require [cljs.test :refer [async deftest is]]
            [frontend.common.thread-api :as thread-api]
            [frontend.worker.a-test-env]
            [frontend.worker.db-core :as db-worker]
            [frontend.worker.local-tx-log :as local-tx-log]
            [frontend.worker.platform :as platform]
            [frontend.worker.shared-service :as shared-service]
            [frontend.worker.state :as worker-state]
            [promesa.core :as p]))

(def ^:private test-repo "test-db-worker-repo")
(def ^:private close-db!-orig db-worker/close-db!)
(def ^:private update-local-tx-orig local-tx-log/update-local-tx)
(def ^:private broadcast-to-clients-orig shared-service/broadcast-to-clients!)
;; Keep this compact to satisfy lint:large-vars while still checking full coverage.
(def ^:private expected-db-core-thread-apis
  (into #{}
        (concat
         [:thread-api/list-db :thread-api/init :thread-api/get-key-value
          :thread-api/create-or-open-db :thread-api/q :thread-api/datoms :thread-api/pull :thread-api/task-spent-time :thread-api/get-blocks
          :thread-api/get-block-refs :thread-api/get-block-source
          :thread-api/get-block-parents :thread-api/set-context :thread-api/transact :thread-api/undo-redo-set-pending-editor-info
          :thread-api/undo-redo-record-editor-info :thread-api/undo-redo-record-ui-state :thread-api/undo-redo-undo
          :thread-api/undo-redo-redo :thread-api/undo-redo-clear-history :thread-api/undo-redo-get-debug-state
          :thread-api/reset-db :thread-api/get-file-content :thread-api/get-all-properties
          :thread-api/get-date-scheduled-or-deadlines
          :thread-api/unsafe-unlink-db :thread-api/close-db
          :thread-api/release-access-handles :thread-api/db-exists
          :thread-api/export-db-binary :thread-api/import-file-graph
          :thread-api/export-client-ops-db-binary :thread-api/backup-db-sqlite
          :thread-api/import-db-binary :thread-api/search-blocks :thread-api/search-upsert-blocks :thread-api/search-delete-blocks
          :thread-api/search-truncate-tables :thread-api/search-build-blocks-indice :thread-api/search-build-blocks-indice-in-worker
          :thread-api/search-build-pages-indice :thread-api/apply-outliner-ops :thread-api/sync-app-state
          :thread-api/markdown-mirror-set-enabled :thread-api/markdown-mirror-flush :thread-api/markdown-mirror-regenerate
          :thread-api/export-get-debug-datoms :thread-api/export-get-all-page->content :thread-api/validate-db
          :thread-api/export-edn :thread-api/import-edn
          :thread-api/get-fsrs-due-card-block-ids :thread-api/get-view-data
          :thread-api/get-class-objects :thread-api/validate-block-tag
          :thread-api/convert-tag-to-page
          :thread-api/convert-page-to-tag
          :thread-api/set-page-favorite
          :thread-api/reorder-favorites
          :thread-api/get-page-route-info :thread-api/query-custom :thread-api/query-dsl-query :thread-api/query-dsl-custom-query
          :thread-api/get-journal-page-by-day :thread-api/get-latest-journals
          :thread-api/page-exists? :thread-api/get-case-page :thread-api/get-tags-by-name
          :thread-api/resolve-query-inputs :thread-api/get-block-parent
          :thread-api/get-block-page-info
          :thread-api/get-block-immediate-children :thread-api/get-block-sibling
          :thread-api/get-page-blocks-tree :thread-api/get-block-class-default-properties
          :thread-api/get-all-classes :thread-api/get-structured-children :thread-api/get-class-extends-children-tree
          :thread-api/get-alias-source-page :thread-api/get-property-closed-values
          :thread-api/get-first-url-property-value
          :thread-api/get-display-properties :thread-api/reorder-display-property
          :thread-api/get-all-properties :thread-api/get-property-values :thread-api/get-bidirectional-properties
          :thread-api/build-graph :thread-api/get-all-page-titles :thread-api/gc-graph :thread-api/mobile-logs
          :thread-api/get-graph-uuid :thread-api/get-rtc-graph-uuid
          :thread-api/ensure-local-graph-uuid
          :thread-api/cli-list-properties :thread-api/cli-list-tags :thread-api/cli-list-pages
          :thread-api/cli-list-tasks :thread-api/cli-list-nodes :thread-api/api-get-page-data :thread-api/api-list-properties
          :thread-api/api-list-tags :thread-api/api-list-pages :thread-api/api-build-upsert-nodes-edn])))

(defn- get-thread-api
  [k]
  (let [f (get @thread-api/*thread-apis k)]
    (assert (fn? f) (str "thread api not registered: " k))
    f))

(defn- fake-db
  [label closed]
  (let [tx #js {:exec (fn [_] nil)}]
    #js {:exec (fn [_] #js [])
         :transaction (fn [f] (f tx))
         :close (fn []
                  (when (and label closed)
                    (swap! closed conj label)))}))

(defn- build-test-platform
  []
  {:env {:publishing? false
         :runtime :browser}
   :storage {:install-opfs-pool (fn [_sqlite _pool-name]
                                  (p/resolved #js {:pauseVfs (fn [] nil)
                                                   :unpauseVfs (fn [] nil)}))
             :list-graphs (fn [] (p/resolved []))
             :db-exists? (fn [_] (p/resolved false))
             :resolve-db-path (fn [_repo _pool path] path)
             :export-file (fn [_ _] (p/resolved (js/Uint8Array. 0)))
             :import-db (fn [_ _ _] (p/resolved nil))
             :remove-vfs! (fn [_] nil)
             :read-text! (fn [_] (p/resolved ""))
             :write-text! (fn [_ _] (p/resolved nil))
             :transfer (fn [data _transferables] data)}
   :kv {:get (fn [_] nil)
        :set! (fn [_ _] nil)}
   :broadcast {:post-message! (fn [& _] nil)}
   :websocket {:connect (fn [_] #js {})}
   :sqlite {:init! (fn [] nil)
            :open-db (fn [_opts] (fake-db nil nil))
            :close-db (fn [db] (.close db))
            :exec (fn [db sql-or-opts] (.exec db sql-or-opts))
            :transaction (fn [db f] (.transaction db f))}
   :crypto {}
   :timers {:set-interval! (fn [_ _] nil)}})

(defn- restoring-worker-state
  [f]
  (let [sqlite-conns-prev @worker-state/*sqlite-conns
        datascript-prev @worker-state/*datascript-conns
        client-ops-prev @worker-state/*client-ops-conns
        opfs-prev @worker-state/*opfs-pools
        sqlite-prev @worker-state/*sqlite
        platform-prev @@#'platform/*platform
        cleanup (fn []
                  (set! db-worker/close-db! close-db!-orig)
                  (set! local-tx-log/update-local-tx update-local-tx-orig)
                  (set! shared-service/broadcast-to-clients! broadcast-to-clients-orig)
                  (reset! worker-state/*sqlite-conns sqlite-conns-prev)
                  (reset! worker-state/*datascript-conns datascript-prev)
                  (reset! worker-state/*client-ops-conns client-ops-prev)
                  (reset! worker-state/*opfs-pools opfs-prev)
                  (reset! worker-state/*sqlite sqlite-prev)
                  (reset! @#'platform/*platform platform-prev))]
    (set! db-worker/close-db! close-db!-orig)
    (set! local-tx-log/update-local-tx update-local-tx-orig)
    (set! shared-service/broadcast-to-clients! broadcast-to-clients-orig)
    (platform/set-platform! (build-test-platform))
    (reset! worker-state/*sqlite #js {})
    (let [result (f)]
      (if (p/promise? result)
        (p/finally result cleanup)
        (do
          (cleanup)
          result)))))

(deftest close-db-clears-worker-state-test
  (restoring-worker-state
   (fn []
     (let [closed (atom [])
           pause-calls (atom 0)
           mk-db (fn [label]
                   #js {:close (fn [] (swap! closed conj label))})]
       (reset! worker-state/*sqlite-conns
               {test-repo {:db (mk-db :db)
                           :search (mk-db :search)
                           :client-ops (mk-db :client-ops)}})
       (reset! worker-state/*datascript-conns {test-repo :datascript})
       (reset! worker-state/*client-ops-conns {test-repo :client-ops})
       (reset! worker-state/*opfs-pools
               {test-repo #js {:pauseVfs (fn [] (swap! pause-calls inc))}})
       (reset! local-tx-log/*repo->pending-local-tx-count {test-repo 9})

       (db-worker/close-db! test-repo)

       (is (= #{:db :search :client-ops} (set @closed)))
       (is (= 1 @pause-calls))
       (is (nil? (get @local-tx-log/*repo->pending-local-tx-count test-repo)))
       (is (nil? (get @worker-state/*sqlite-conns test-repo)))))))

(deftest close-db-checkpoints-wal-before-closing-test
  (restoring-worker-state
   (fn []
     (let [sql-calls (atom [])
           closed (atom [])
           mk-db (fn [label]
                   #js {:exec (fn [sql]
                                (swap! sql-calls conj [label sql]))
                        :close (fn []
                                 (swap! closed conj label))})]
       (reset! worker-state/*sqlite-conns
               {test-repo {:db (mk-db :db)
                           :search (mk-db :search)
                           :client-ops (mk-db :client-ops)}})

       (db-worker/close-db! test-repo)

       (is (= [[:db "PRAGMA wal_checkpoint(TRUNCATE)"]
               [:search "PRAGMA wal_checkpoint(TRUNCATE)"]
               [:client-ops "PRAGMA wal_checkpoint(TRUNCATE)"]]
              @sql-calls))
       (is (= [:db :search :client-ops] @closed))))))

(deftest client-ops-cleanup-timer-starts-once-and-clears-on-close-test
  (restoring-worker-state
   (fn []
     (let [scheduled (atom [])
           cleared (atom [])
           original-set-interval js/setInterval
           original-clear-interval js/clearInterval
           fake-db' #js {:close (fn [] nil)}
           timer-id #js {:id "timer-1"}]
       (set! js/setInterval
             (fn [f interval-ms]
               (swap! scheduled conj {:fn f :interval-ms interval-ms})
               timer-id))
       (set! js/clearInterval
             (fn [id]
               (swap! cleared conj id)))
       (try
         (reset! worker-state/*sqlite-conns
                 {test-repo {:db fake-db'
                             :search fake-db'
                             :client-ops fake-db'}})
         (reset! worker-state/*datascript-conns {test-repo :datascript})
         (reset! worker-state/*client-ops-conns {test-repo :client-ops})
         (reset! (deref #'db-worker/*client-ops-cleanup-timers) {})

         (#'db-worker/ensure-client-ops-cleanup-timer! test-repo)
         (#'db-worker/ensure-client-ops-cleanup-timer! test-repo)

         (is (= 1 (count @scheduled)))
         (is (= (* 3 60 60 1000) (:interval-ms (first @scheduled))))
         (is (= timer-id (get @(deref #'db-worker/*client-ops-cleanup-timers) test-repo)))

         (db-worker/close-db! test-repo)

         (is (= [timer-id] @cleared))
         (is (nil? (get @(deref #'db-worker/*client-ops-cleanup-timers) test-repo)))
         (finally
           (set! js/setInterval original-set-interval)
           (set! js/clearInterval original-clear-interval)))))))

(deftest thread-api-export-client-ops-db-binary-checkpoints-and-exports-client-ops-file-test
  (async done
    (restoring-worker-state
     (fn []
       (let [export-client-ops-db-binary (@thread-api/*thread-apis :thread-api/export-client-ops-db-binary)
             sql-calls (atom [])
             export-calls (atom [])
             expected-data (js/Uint8Array. #js [1 2 3])
             expected-buffer (.-buffer expected-data)
             fake-pool #js {}
             platform' (assoc-in (build-test-platform)
                                 [:storage :export-file]
                                 (fn [_pool path]
                                   (swap! export-calls conj path)
                                   (p/resolved expected-buffer)))]
         (platform/set-platform! platform')
         (reset! worker-state/*opfs-pools {test-repo fake-pool})
         (with-redefs [worker-state/get-sqlite-conn (fn [_repo which-db]
                                                      (when (= :client-ops which-db)
                                                        #js {:exec (fn [sql]
                                                                     (swap! sql-calls conj sql))}))]
           (-> (export-client-ops-db-binary test-repo)
               (p/then (fn [result]
                         (is (= ["PRAGMA wal_checkpoint(TRUNCATE)"] @sql-calls))
                         (is (= 1 (count @export-calls)))
                         (is (contains? #{"client-ops/db.sqlite"
                                          "client-ops-/db.sqlite"}
                                        (first @export-calls)))
                         (is (instance? js/Uint8Array result))
                         (is (= [1 2 3]
                                (vec result)))
                         (done)))
               (p/catch (fn [error]
                          (is false (str error))
                          (done))))))))))

(deftest thread-api-export-client-ops-db-binary-supports-flat-client-ops-filename-test
  (async done
    (restoring-worker-state
     (fn []
       (let [export-client-ops-db-binary (@thread-api/*thread-apis :thread-api/export-client-ops-db-binary)
             sql-calls (atom [])
             export-calls (atom [])
             expected-data (js/Uint8Array. #js [9 8 7])
             expected-buffer (.-buffer expected-data)
             fake-pool #js {}
             platform' (assoc-in (build-test-platform)
                                 [:storage :export-file]
                                 (fn [_pool path]
                                   (swap! export-calls conj path)
                                   (if (= "client-ops-db.sqlite" path)
                                     (p/resolved expected-buffer)
                                     (p/rejected (ex-info "missing path" {:path path})))))]
         (platform/set-platform! platform')
         (reset! worker-state/*opfs-pools {test-repo fake-pool})
         (with-redefs [worker-state/get-sqlite-conn (fn [_repo which-db]
                                                      (when (= :client-ops which-db)
                                                        #js {:exec (fn [sql]
                                                                     (swap! sql-calls conj sql))}))]
           (-> (export-client-ops-db-binary test-repo)
               (p/then (fn [result]
                         (is (= ["PRAGMA wal_checkpoint(TRUNCATE)"] @sql-calls))
                         (is (contains? (set @export-calls) "client-ops-db.sqlite"))
                         (is (instance? js/Uint8Array result))
                         (is (= [9 8 7]
                                (vec result)))
                         (done)))
               (p/catch (fn [error]
                          (is false (str error))
                          (done))))))))))

(deftest thread-api-export-client-ops-db-binary-supports-normalized-browser-client-ops-path-test
  (async done
    (restoring-worker-state
     (fn []
       (let [export-client-ops-db-binary (@thread-api/*thread-apis :thread-api/export-client-ops-db-binary)
             export-calls (atom [])
             expected-data (js/Uint8Array. #js [4 5 6])
             expected-buffer (.-buffer expected-data)
             fake-pool #js {}
             platform' (assoc-in (build-test-platform)
                                 [:storage :export-file]
                                 (fn [_pool path]
                                   (swap! export-calls conj path)
                                   (if (= "/client-ops-/db.sqlite" path)
                                     (p/resolved expected-buffer)
                                     (p/rejected (ex-info "missing path" {:path path})))))]
         (platform/set-platform! platform')
         (reset! worker-state/*opfs-pools {test-repo fake-pool})
         (with-redefs [worker-state/get-sqlite-conn (fn [_repo which-db]
                                                      (when (= :client-ops which-db)
                                                        #js {:exec (fn [_sql] nil)}))]
           (-> (export-client-ops-db-binary test-repo)
               (p/then (fn [result]
                         (is (contains? (set @export-calls) "/client-ops-/db.sqlite"))
                         (is (instance? js/Uint8Array result))
                         (is (= [4 5 6]
                                (vec result)))
                         (done)))
               (p/catch (fn [error]
                          (is false (str error))
                          (done))))))))))

(deftest thread-api-db-core-registers-all-thread-apis-test
  (let [missing (->> expected-db-core-thread-apis
                     (remove #(contains? @thread-api/*thread-apis %))
                     sort
                     vec)
        non-functions (->> expected-db-core-thread-apis
                           (filter #(not (fn? (get @thread-api/*thread-apis %))))
                           sort
                           vec)]
    (is (empty? missing) (str "Missing thread apis: " missing))
    (is (empty? non-functions) (str "Non-function thread apis: " non-functions))))

(deftest thread-api-release-access-handles-pauses-vfs-test
  (restoring-worker-state
   (fn []
     (let [repo test-repo
           pause-calls (atom [])
           pool #js {:pauseVfs (fn []
                                 (swap! pause-calls conj :paused))}]
       (reset! worker-state/*opfs-pools {repo pool})
       (is (nil? ((get-thread-api :thread-api/release-access-handles) repo)))
       (is (= [:paused] @pause-calls))))))
