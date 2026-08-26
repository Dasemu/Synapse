(ns frontend.worker.mcp-server
  "MCP server for the daemon (Fase 9), exposing the semantic API
  (semantic-routes/operations) as MCP tools. The CF-Worker's equivalent
  (deps/db-sync/worker/entry.mjs) auto-generates its tools from the OpenAPI
  document via @cloudflare/codemode's openApiMcpServer, a package that only
  exists inside Cloudflare Workers -- not portable to Node -- plus a pile of
  ChatGPT-App-specific tooling (asset image resources, upload_asset,
  Cognito protected-resource metadata) this daemon has no use for. So this
  registers one tool per operation by hand instead, the same pattern
  electron/mcp_server.cljs already used for its own 6 tools (registerTool
  with an explicit title/description/inputSchema/handler) -- just generated
  from semantic-routes' operation table and docs instead of writing 34
  one-off registrations, so the MCP tool surface can't drift from the HTTP
  routes and the OpenAPI document.

  Each tool's handler makes a loopback HTTP call to this same daemon's own
  /v1/semantic/* route (see db_worker_node.cljs), forwarding the
  Authorization header the /mcp request itself carried -- reusing the
  already-tested HTTP-layer logic (repo-error validation, lock-owner
  assertion, response shaping) rather than re-implementing a second
  request path directly against the thread-api."
  (:require ["@modelcontextprotocol/sdk/server/mcp.js" :refer [McpServer]]
            ["@modelcontextprotocol/sdk/server/streamableHttp.js" :refer [StreamableHTTPServerTransport]]
            ["@modelcontextprotocol/sdk/types.js" :refer [isInitializeRequest]]
            ["zod/v3" :as z]
            [clojure.string :as string]
            [frontend.worker.semantic-routes :as semantic-routes]
            [lambdaisland.glogi :as log]
            [promesa.core :as p]))

;; Shared Zod field fragments
;; ==========================
(def ^:private pagination-fields
  {:limit (-> (z/number) .int .optional (.describe "Max items per page (default 50, max 200)"))
   :cursor (-> (z/string) .optional (.describe "Opaque pagination cursor from a previous response's next-cursor"))})

(def ^:private time-filter-fields
  {:created-after (-> (z/number) .int .optional (.describe "Unix epoch ms; only entities created strictly after this"))
   :updated-after (-> (z/number) .int .optional (.describe "Unix epoch ms; only entities updated strictly after this"))})

(def ^:private listing-fields (merge pagination-fields time-filter-fields))

(def ^:private block-tree-schema
  "Self-recursive, so it needs z/lazy -- the thunk closes over this same var,
  resolved once def completes, the standard cljs recursive-def idiom."
  (z/lazy (fn []
            (z/object #js {:title (z/string)
                           :children (-> (z/array block-tree-schema) .optional)}))))

(def ^:private property-value-schema
  (-> (z/union #js [(z/string) (z/number) (z/boolean)
                    (z/array (z/union #js [(z/string) (z/number)]))])
      (.describe (str "For ref properties, an existing entity's UUID, ident, or title. Status/Priority also "
                      "accept built-in aliases like TODO or HIGH. Class properties like Tags accept one "
                      "selector or an array of them. Scalar properties take their JSON value directly."))))

(def ^:private task-status-field
  (-> (z/string) .optional
      (.describe "todo|doing|in-review|done|canceled|backlog, or an existing Status choice's UUID/ident/title")))

(def ^:private task-priority-field
  (-> (z/string) .optional
      (.describe "low|medium|high|urgent, or an existing Priority choice's UUID/ident/title")))

(def ^:private property-type-field
  (-> (z/enum #js ["default" "number" "date" "datetime" "checkbox" "url" "node" "asset" "map" "json" "string"])
      .optional))

(def ^:private property-cardinality-field
  (-> (z/enum #js ["db.cardinality/one" "db.cardinality/many"]) .optional))

;; Per-operation extra fields (beyond the path params every operation gets
;; automatically -- see path-arg-fields below)
;; ============================================================================
(def ^:private extra-fields
  {"listPages" listing-fields
   "createPage" {:title (z/string)}
   "listPageBlocks" listing-fields
   "listPageReferences" listing-fields
   "updatePage" {:title (z/string)}
   "updateBlock" {:title (z/string)}
   "moveBlocks" {:block-ids (z/array (z/string))
                 :target-id (z/string)
                 :position (z/enum #js ["before" "after" "first-child" "last-child"])}
   "insertBlockChildren" {:position (z/enum #js ["append" "prepend"])
                          :blocks (z/array block-tree-schema)}
   "insertBlockTree" {:target-id (z/string)
                      :position (-> (z/enum #js ["append" "prepend"]) .optional)
                      :blocks (z/array block-tree-schema)}
   "setBlockProperty" {:value property-value-schema}
   "batchSetBlockProperty" {:isResetExistingValues (-> (z/boolean) .optional)
                            :entries (z/array (z/object #js {:block-id (z/string)
                                                              :property-id (z/string)
                                                              :value property-value-schema}))}
   "batchDeleteBlockProperty" {:entries (z/array (z/object #js {:block-id (z/string) :property-id (z/string)}))}
   "captureToToday" {:blocks (z/array block-tree-schema)}
   "listTasks" (merge listing-fields
                       {:status task-status-field
                        :priority task-priority-field
                        :content (-> (z/string) .optional (.describe "Case-insensitive task title filter"))})
   "createTask" {:title (z/string)
                 :page-id (-> (z/string) .optional (.describe "Destination page UUID; omit for today's journal"))
                 :status task-status-field
                 :priority task-priority-field
                 :scheduled (-> (z/number) .int .optional (.describe "Scheduled date as YYYYMMDD"))
                 :deadline (-> (z/number) .int .optional (.describe "Deadline date as YYYYMMDD"))}
   "listTags" listing-fields
   "createTag" {:title (z/string)}
   "listTagObjects" listing-fields
   "updateTag" {:title (z/string)}
   "listProperties" listing-fields
   "createProperty" {:title (z/string) :type property-type-field :cardinality property-cardinality-field}
   "updateProperty" {:title (-> (z/string) .optional) :type property-type-field :cardinality property-cardinality-field}
   "listAssets" listing-fields
   "createAsset" {:file-name (z/string)
                  :size (-> (z/number) .int (.describe "Exact decoded byte size of the file, max 100MB"))
                  :checksum (-> (z/string) (.regex (js/RegExp. "^[0-9a-fA-F]{64}$"))
                                (.describe "SHA-256 hex digest of the decoded file"))
                  :title (-> (z/string) .optional)
                  :page-id (-> (z/string) .optional (.describe "Destination page UUID; omit for today's journal"))
                  :content (-> (z/string) (.describe "Base64-encoded file contents"))}
   "searchGraph" (merge listing-fields
                        {:q (z/string)
                         :types (-> (z/string) .optional
                                    (.describe "Comma-separated subset of blocks,tags,properties,assets"))})})

;; Request construction
;; ====================
(defn- path-param-names [path]
  (mapv second (re-seq #":([^/]+)" path)))

(defn- path-arg-fields [path]
  (into {}
        (map (fn [param]
               [(keyword param) (-> (z/string) (.describe (str "The " param)))]))
        (path-param-names path)))

(defn- build-path [path args]
  (reduce (fn [p param]
            (string/replace p (str ":" param) (js/encodeURIComponent (str (get args (keyword param))))))
          path
          (path-param-names path)))

(defn- query-string [args]
  (let [pairs (->> args
                   (remove (fn [[_ v]] (nil? v)))
                   (map (fn [[k v]] (str (js/encodeURIComponent (name k)) "=" (js/encodeURIComponent (str v))))))]
    (if (seq pairs) (str "?" (string/join "&" pairs)) "")))

(defn- request-for
  [{:keys [operation-id method path]} args]
  (let [path-names (set (map keyword (path-param-names path)))
        rest-args (apply dissoc args path-names)
        url-path (build-path path args)]
    (cond
      (= operation-id "createAsset")
      {:method method
       :path (str url-path (query-string (assoc (dissoc rest-args :content) :encoding "base64")))
       :body (:content rest-args)
       :content-type "text/plain"}

      (contains? #{"GET" "DELETE"} method)
      {:method method :path (str url-path (query-string rest-args)) :body nil :content-type nil}

      :else
      {:method method :path url-path :body (js/JSON.stringify (clj->js rest-args)) :content-type "application/json"})))

(defn- call-semantic-operation!
  [{:keys [host port authorization]} operation args]
  (let [{:keys [method path body content-type]} (request-for operation args)
        url (str "http://" host ":" port path)]
    (-> (js/fetch url (clj->js (cond-> {:method method
                                        :headers (cond-> {}
                                                   authorization (assoc "Authorization" authorization)
                                                   content-type (assoc "Content-Type" content-type))}
                                 body (assoc :body body))))
        (p/then (fn [^js res]
                  (p/let [text (.text res)]
                    {:status (.-status res) :ok (.-ok res) :text text}))))))

(defn- mcp-result [{:keys [ok text]}]
  #js {:content #js [#js {:type "text" :text (if (string/blank? text) "{}" text)}]
       :isError (not ok)})

(defn- mcp-error-result [error]
  #js {:content #js [#js {:type "text" :text (str "Unexpected MCP tool error: " (or (.-message error) error))}]
       :isError true})

;; Server construction
;; ===================
(defn- register-operation-tool!
  [^js mcp-server ctx operation]
  (let [{:keys [operation-id]} operation
        [title description] (get semantic-routes/operation-docs operation-id)
        input-shape (clj->js (merge (path-arg-fields (:path operation)) (get extra-fields operation-id {})))]
    (.registerTool mcp-server operation-id
                   #js {:title title
                        :description description
                        :inputSchema input-shape}
                   (fn [args]
                     (-> (call-semantic-operation! ctx operation (js->clj args :keywordize-keys true))
                         (p/then mcp-result)
                         (p/catch mcp-error-result))))))

(defn create-mcp-api-server
  [ctx]
  (let [mcp-server (McpServer. #js {:name "Logseq Daemon Semantic API" :version "1.0.0"})]
    (doseq [operation semantic-routes/operations]
      (register-operation-tool! mcp-server ctx operation))
    mcp-server))

;; HTTP glue -- adapted from electron/mcp_server.cljs's Fastify-based version:
;; that one gets req/res already wrapped (`.raw` unwraps back to the
;; underlying Node primitives, `.body` is pre-parsed by Fastify's body
;; parser); the daemon calls this straight from its own raw
;; http.createServer callback, so req/res ARE already the Node primitives
;; StreamableHTTPServerTransport wants, and the body needs parsing here.
;; Auth is the daemon's existing bearer-token check on every route (see
;; db_worker_node.cljs's request-authorized?) -- already done by the time
;; this is reached, so unlike the CF-Worker/Electron versions there's no
;; separate auth check in here.
(defonce ^:private *transports (atom {}))

(defn- <read-body [^js req]
  (p/create
   (fn [resolve reject]
     (let [chunks (array)]
       (.on req "data" (fn [chunk] (.push chunks chunk)))
       (.on req "end" (fn [] (resolve (.toString (js/Buffer.concat chunks) "utf8"))))
       (.on req "error" reject)))))

(defn handle-post-request
  [{:keys [host port]} ^js req ^js res]
  (let [session-id (aget (.-headers req) "mcp-session-id")
        authorization (.. req -headers -authorization)]
    (p/let [body-text (<read-body req)
            body (when-not (string/blank? body-text) (js/JSON.parse body-text))]
      (cond
        (and session-id (@*transports session-id))
        (let [^js transport (@*transports session-id)]
          (.handleRequest transport req res body))

        (and (not session-id) (isInitializeRequest body))
        (let [transport (StreamableHTTPServerTransport.
                         #js {:sessionIdGenerator (comp str random-uuid)
                              :enableDnsRebindingProtection true
                              :allowedHosts #js [(str host ":" port)]})
              mcp-server (create-mcp-api-server {:host host :port port :authorization authorization})]
          (set! (.-onclose transport)
                (fn [] (swap! *transports dissoc (.-sessionId transport))))
          (p/do!
           (.connect mcp-server transport)
           (.handleRequest transport req res body)
           (if (.-sessionId transport)
             (swap! *transports assoc (.-sessionId transport) transport)
             (log/error :mcp-server-no-session-id-after-initialize nil))))

        :else
        (do
          (.writeHead res 400 #js {"Content-Type" "application/json"})
          (.end res (js/JSON.stringify
                     #js {:jsonrpc "2.0"
                          :error #js {:code -32000 :message "Bad Request: No valid session ID provided"}
                          :id nil})))))))

(defn handle-get-request
  [^js req ^js res]
  (let [session-id (aget (.-headers req) "mcp-session-id")]
    (if-let [^js transport (and session-id (@*transports session-id))]
      (.handleRequest transport req res)
      (do (.writeHead res 400) (.end res "Invalid or missing session ID")))))

(defn handle-delete-request
  [^js req ^js res]
  (let [session-id (aget (.-headers req) "mcp-session-id")]
    (if-let [^js transport (and session-id (@*transports session-id))]
      (do (.close transport)
          (.writeHead res 200 #js {"Content-Type" "application/json"})
          (.end res (js/JSON.stringify #js {:ok true})))
      (do (.writeHead res 400) (.end res "Invalid or missing session ID")))))
