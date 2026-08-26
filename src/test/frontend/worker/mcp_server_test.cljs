(ns frontend.worker.mcp-server-test
  "Exercises the daemon's /mcp endpoint (Fase 9) with a real MCP client SDK
  connection over HTTP against a live daemon -- the same live-daemon
  pattern db_worker_node_test.cljs and semantic_test.cljs use."
  (:require ["@modelcontextprotocol/sdk/client/index.js" :refer [Client]]
            ["@modelcontextprotocol/sdk/client/streamableHttp.js" :refer [StreamableHTTPClientTransport]]
            [cljs.test :refer [async deftest is]]
            [frontend.test.node-helper :as node-helper]
            [frontend.worker.db-worker-node :as db-worker-node]
            [frontend.worker.semantic-routes :as semantic-routes]
            [promesa.core :as p]))

;; Same literal credential db_worker_node_test.cljs and semantic_test.cljs
;; bootstrap with -- see semantic_test.cljs for why this must not be a
;; separate value (all three namespaces share one test.js process).
(def ^:private test-daemon-username "db-worker-node-test-user")
(def ^:private test-daemon-password "db-worker-node-test-password")

(aset (.-env js/process) "LOGSEQ_DAEMON_USERNAME" test-daemon-username)
(aset (.-env js/process) "LOGSEQ_DAEMON_PASSWORD" test-daemon-password)

(defn- start-daemon!
  [opts]
  (db-worker-node/start-daemon! (update opts :log-level #(or % "error"))))

(defn- <token!
  [host port]
  (p/let [^js res (js/fetch (str "http://" host ":" port "/v1/auth/login")
                            #js {:method "POST"
                                 :headers #js {"Content-Type" "application/json"}
                                 :body (js/JSON.stringify
                                        #js {:username test-daemon-username
                                             :password test-daemon-password
                                             :deviceLabel "mcp-test"})})
          ^js body (.json res)]
    (.-token body)))

(defn- <connected-client!
  ^js
  [host port token]
  (p/let [transport (StreamableHTTPClientTransport.
                     (js/URL. (str "http://" host ":" port "/mcp"))
                     #js {:requestInit #js {:headers #js {"Authorization" (str "Bearer " token)}}})
          ^js client (Client. #js {:name "semantic-mcp-test" :version "1.0.0"})
          _ (.connect client transport)]
    client))

(defn- tool-text
  [^js call-result]
  (.-text (aget (.-content call-result) 0)))

(defn- with-mcp-client!
  "Starts a fresh daemon over a fresh repo, connects an MCP client to /mcp,
  runs (f client), and stops the daemon afterwards regardless of outcome."
  [f]
  (let [daemon (atom nil)
        data-dir (node-helper/create-tmp-dir "mcp-server")
        repo (str "logseq_db_mcp_" (subs (str (random-uuid)) 0 8))]
    (-> (p/let [{:keys [host port stop!]} (start-daemon! {:root-dir data-dir :repo repo})
                _ (reset! daemon {:stop! stop!})
                token (<token! host port)
                client (<connected-client! host port token)]
          (f client))
        (p/catch (fn [e]
                   (println "[mcp-server-test] error:" e)
                   (is false (str e))))
        (p/finally (fn []
                     (when-let [stop! (:stop! @daemon)]
                       (stop!)))))))

(deftest mcp-lists-all-semantic-tools
  (async done
         (-> (with-mcp-client!
              (fn [^js client]
                (p/let [^js result (.listTools client)]
                  (let [tools (js->clj (.-tools result) :keywordize-keys true)]
                    (is (= (count semantic-routes/operations) (count tools)))
                    (is (some #(= "createPage" (:name %)) tools))
                    (is (some #(= "searchGraph" (:name %)) tools))
                    (is (not (some #(= "listGraphs" (:name %)) tools)))))))
             (p/finally done))))

(deftest mcp-create-and-get-page-round-trip
  (async done
         (-> (with-mcp-client!
              (fn [^js client]
                (p/let [^js create-result (.callTool client #js {:name "createPage"
                                                                  :arguments #js {:title "MCP Test Page"}})
                        page (js->clj (js/JSON.parse (tool-text create-result)) :keywordize-keys true)
                        ^js get-result (.callTool client #js {:name "getPage"
                                                               :arguments #js {:page-id (:uuid page)}})
                        got (js->clj (js/JSON.parse (tool-text get-result)) :keywordize-keys true)]
                  (is (not (.-isError create-result)))
                  (is (= "MCP Test Page" (:title page)))
                  (is (= "MCP Test Page" (:title got))))))
             (p/finally done))))

(deftest mcp-capture-block-tree-round-trip
  (async done
         (-> (with-mcp-client!
              (fn [^js client]
                (p/let [^js result (.callTool
                                    client #js {:name "captureToToday"
                                                :arguments #js {:blocks #js [#js {:title "MCP root"
                                                                                  :children #js [#js {:title "MCP child"}]}]}})
                        body (js->clj (js/JSON.parse (tool-text result)) :keywordize-keys true)
                        root (first (:blocks body))]
                  (is (not (.-isError result)))
                  (is (= "MCP root" (:title root)))
                  (is (= "MCP child" (:title (first (:children root))))))))
             (p/finally done))))

(deftest mcp-unknown-page-surfaces-as-error-content
  (async done
         (-> (with-mcp-client!
              (fn [^js client]
                (p/let [^js result (.callTool client #js {:name "getPage"
                                                           :arguments #js {:page-id (str (random-uuid))}})]
                  (is (true? (.-isError result))))))
             (p/finally done))))
