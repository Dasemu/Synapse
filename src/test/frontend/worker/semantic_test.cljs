(ns frontend.worker.semantic-test
  "Exercises the semantic API (Fase 9) over real HTTP against a live daemon,
  the same way db_worker_node_test.cljs exercises /v1/invoke -- these
  operations were ported from deps/db-sync/.../handler/semantic.cljs onto
  frontend.worker.handler.semantic, re-hosted on the daemon's own
  authoritative conn instead of a Cloudflare Durable Object's SQLite."
  (:require ["http" :as http]
            [cljs.test :refer [async deftest is]]
            [frontend.test.node-helper :as node-helper]
            [frontend.worker.db-worker-node :as db-worker-node]))

;; Same literal credential db_worker_node_test.cljs bootstraps with -- both
;; namespaces get `require`d into the same test.js bundle, and this is a
;; process-wide env var, so setting it to a different value here would win
;; the race against whichever namespace loads later and lock the other
;; file's daemons out of their own bootstrapped credential.
(def ^:private test-daemon-username "db-worker-node-test-user")
(def ^:private test-daemon-password "db-worker-node-test-password")

(aset (.-env js/process) "LOGSEQ_DAEMON_USERNAME" test-daemon-username)
(aset (.-env js/process) "LOGSEQ_DAEMON_PASSWORD" test-daemon-password)

(defn- http-request
  [opts body]
  (js/Promise.
   (fn [resolve reject]
     (let [req (.request http (clj->js opts)
                         (fn [^js res]
                           (let [chunks (array)]
                             (.on res "data" (fn [chunk] (.push chunks chunk)))
                             (.on res "end" (fn []
                                              (resolve {:status (.-statusCode res)
                                                        :body (.toString (js/Buffer.concat chunks) "utf8")}))))))]
       (.on req "error" reject)
       (when body (.write req body))
       (.end req)))))

(defn- json-body [{:keys [body]}]
  (when-not (= "" body)
    (js->clj (js/JSON.parse body) :keywordize-keys true)))

(defn- auth-headers!
  [host port]
  (.then (http-request {:hostname host
                        :port port
                        :path "/v1/auth/login"
                        :method "POST"
                        :headers {"Content-Type" "application/json"}}
                       (js/JSON.stringify
                        (clj->js {:username test-daemon-username
                                  :password test-daemon-password
                                  :deviceLabel "test"})))
        (fn [result]
          {"Content-Type" "application/json"
           "Authorization" (str "Bearer " (:token (json-body result)))})))

(defn- semantic-request
  ([host port method path] (semantic-request host port method path nil))
  ([host port method path body]
   (.then (auth-headers! host port)
          (fn [headers]
            (http-request {:hostname host
                           :port port
                           :path path
                           :method method
                           :headers headers}
                          (when body (js/JSON.stringify (clj->js body))))))))

(defn- start-daemon!
  [opts]
  (db-worker-node/start-daemon! (update opts :log-level #(or % "error"))))

(defn- with-daemon!
  "Starts a fresh daemon over a fresh repo, runs (f host port), and stops the
  daemon afterwards regardless of outcome. Returns a promise for `async`."
  [f]
  (let [daemon (atom nil)
        data-dir (node-helper/create-tmp-dir "semantic-api")
        repo (str "logseq_db_semantic_" (subs (str (random-uuid)) 0 8))]
    (-> (.then (start-daemon! {:root-dir data-dir :repo repo})
               (fn [{:keys [host port stop!]}]
                 (reset! daemon {:stop! stop!})
                 (f host port)))
        (.catch (fn [e]
                  (println "[semantic-test] error:" e)
                  (is false (str e))))
        (.finally (fn []
                    (when-let [stop! (:stop! @daemon)]
                      (stop!)))))))

(deftest semantic-pages-create-get-list-and-delete
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (semantic-request host port "POST" "/v1/semantic/pages" {:title "Semantic Test Page"})
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 201 status))
                             (let [page (json-body result)]
                               (is (= "Semantic Test Page" (:title page)))
                               (is (= "page" (:kind page)))
                               (.then (semantic-request host port "GET" (str "/v1/semantic/pages/" (:uuid page)))
                                      (fn [{:keys [status] :as get-result}]
                                        (is (= 200 status))
                                        (is (= "Semantic Test Page" (:title (json-body get-result))))
                                        page)))))
                    (.then (fn [page]
                             (.then (semantic-request host port "GET" "/v1/semantic/pages")
                                    (fn [{:keys [status] :as list-result}]
                                      (is (= 200 status))
                                      (is (some #(= (:uuid page) (:uuid %)) (:blocks (json-body list-result))))
                                      page))))
                    (.then (fn [page]
                             (.then (semantic-request host port "DELETE" (str "/v1/semantic/pages/" (:uuid page)))
                                    (fn [{:keys [status]}]
                                      (is (= 204 status))
                                      (semantic-request host port "GET" (str "/v1/semantic/pages/" (:uuid page)))))))
                    (.then (fn [{:keys [status]}]
                             (is (= 404 status)))))))
             (.finally done))))

(deftest semantic-blocks-property-and-capture-flow
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (semantic-request host port "POST" "/v1/semantic/capture" {:blocks [{:title "Captured root"
                                                                                          :children [{:title "child"}]}]})
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 201 status))
                             (let [body (json-body result)
                                   root (first (:blocks body))]
                               (is (= "Captured root" (:title root)))
                               (is (= 1 (count (:children root))))
                               root)))
                    (.then (fn [root]
                             (.then (semantic-request host port "POST" "/v1/semantic/properties"
                                                      {:title "semantic-test-prop" :type "string"
                                                       :cardinality "db.cardinality/one"})
                                    (fn [{:keys [status] :as property-result}]
                                      (is (= 201 status))
                                      [root (json-body property-result)]))))
                    (.then (fn [[root property]]
                             (.then (semantic-request host port "PUT"
                                                      (str "/v1/semantic/blocks/" (:uuid root)
                                                           "/properties/" (:uuid property))
                                                      {:value "hello"})
                                    (fn [{:keys [status] :as set-result}]
                                      (is (= 200 status))
                                      (is (:updated (json-body set-result)))
                                      [root property]))))
                    (.then (fn [[root property]]
                             (semantic-request host port "DELETE"
                                               (str "/v1/semantic/blocks/" (:uuid root)
                                                    "/properties/" (:uuid property)))))
                    (.then (fn [{:keys [status]}]
                             (is (= 204 status)))))))
             (.finally done))))

(deftest semantic-tags-and-tasks-flow
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (semantic-request host port "POST" "/v1/semantic/tags" {:title "Semantic Tag"})
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 201 status))
                             (is (= "Semantic Tag" (:title (json-body result))))))
                    (.then (fn [_]
                             (semantic-request host port "POST" "/v1/semantic/tasks"
                                               {:title "Semantic Task" :status "doing" :priority "high"})))
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 201 status))
                             (let [task (json-body result)]
                               (is (= "logseq.property/status.doing" (get-in task [:status :ident])))
                               (is (= "logseq.property/priority.high" (get-in task [:priority :ident]))))))
                    (.then (fn [_]
                             (semantic-request host port "GET" "/v1/semantic/tasks?status=doing")))
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 200 status))
                             (is (some #(= "Semantic Task" (:title %)) (:tasks (json-body result)))))))))
             (.finally done))))

(deftest semantic-search-finds-created-page
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (semantic-request host port "POST" "/v1/semantic/pages" {:title "Findable Semantic Page"})
                    (.then (fn [_]
                             (semantic-request host port "GET" "/v1/semantic/search?q=Findable")))
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 200 status))
                             (is (some #(= "Findable Semantic Page" (:title %)) (:results (json-body result)))))))))
             (.finally done))))

(deftest semantic-request-requires-auth
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (http-request {:hostname host :port port :path "/v1/semantic/pages" :method "GET"} nil)
                    (.then (fn [{:keys [status]}]
                             (is (= 401 status)))))))
             (.finally done))))

(deftest semantic-unknown-route-is-not-found
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (semantic-request host port "GET" "/v1/semantic/not-a-real-route")
                    (.then (fn [{:keys [status]}]
                             (is (= 404 status)))))))
             (.finally done))))

(deftest semantic-openapi-document-lists-operations
  (async done
         (-> (with-daemon!
              (fn [host port]
                (-> (semantic-request host port "GET" "/v1/semantic/openapi.json")
                    (.then (fn [{:keys [status] :as result}]
                             (is (= 200 status))
                             (let [doc (json-body result)]
                               (is (= "3.1.0" (:openapi doc)))
                               (is (contains? (:paths doc) (keyword "/v1/semantic/pages")))
                               (is (not (contains? (:paths doc) (keyword "/v1/graphs/{graph-id}/semantic/pages"))))))))))
             (.finally done))))
