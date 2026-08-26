(ns frontend.persist-db.remote
  "Remote `PersistentDB` implementation for Electron renderer via db-worker-node HTTP and SSE."
  (:require [clojure.string :as string]
            [frontend.db.subs :as db-subs]
            [frontend.persist-db.protocol :as protocol]
            [logseq.db :as ldb]
            [promesa.core :as p]
            [lambdaisland.glogi :as log]
            [frontend.handler.notification :as notification]
            [frontend.context.i18n :refer [t]]))

(defn- normalize-base-url
  [base-url]
  (string/replace (or base-url "") #"/$" ""))

(defn- invoke-url
  [base-url]
  (str (normalize-base-url base-url) "/v1/invoke"))

(defn- invoke-batch-url
  [base-url]
  (str (normalize-base-url base-url) "/v1/invoke-batch"))

(defn- import-db-binary-url
  [base-url repo]
  (str (normalize-base-url base-url)
       "/v1/import-db-binary?repo="
       (js/encodeURIComponent repo)))

(defn- asset-url
  [base-url repo file-name]
  (str (normalize-base-url base-url)
       "/v1/assets/"
       (js/encodeURIComponent repo)
       "/"
       (js/encodeURIComponent file-name)))

(defn- events-url
  "EventSource has no header API, so the bearer token travels as a query
  param here instead of the `Authorization` header every other request
  uses -- the server accepts it for this route only (see
  request-authorized? in db_worker_node.cljs). Interim until the Fase 4
  WebSocket migration, which carries it via Sec-WebSocket-Protocol instead."
  [base-url auth-token]
  (cond-> (str (normalize-base-url base-url) "/v1/events")
    (seq auth-token) (str "?token=" (js/encodeURIComponent auth-token))))

(defn- events-ws-url
  [base-url]
  (-> (normalize-base-url base-url)
      (string/replace #"^http" "ws")
      (str "/v1/events")))

(defn- base-headers
  [auth-token]
  (cond-> {"Content-Type" "application/json"
           "Accept" "application/json"}
    (seq auth-token)
    (assoc "Authorization" (str "Bearer " auth-token))))

(defn- binary-headers
  [auth-token]
  (assoc (base-headers auth-token) "Content-Type" "application/octet-stream"))

(defn- parse-response-body
  [body]
  (cond
    (string? body) (js->clj (js/JSON.parse body) :keywordize-keys true)
    (map? body) body
    :else {}))

(defn- normalize-code
  [code]
  (cond
    (keyword? code) code
    (string? code) (keyword code)
    :else :invoke-failed))

(defn- login-url
  [base-url]
  (str (normalize-base-url base-url) "/v1/auth/login"))

(defn- default-fetch-json!
  [{:keys [method url headers body]}]
  (p/let [^js res (js/fetch url (clj->js (cond-> {:method method
                                                  :headers (or headers {})}
                                           body (assoc :body body))))
          text (.text res)]
    {:status (.-status res)
     :body text}))

(defn login!
  "Logs into a db-worker-node daemon's HTTP auth (see
  frontend.worker.db-worker-node-auth) and returns {:token :session-id} on
  success. Used by the web/mobile server-connection settings UI; Electron
  gets its token from the main process instead (see
  frontend.persist-db/<resolve-remote-runtime!)."
  [{:keys [base-url username password device-label fetch-fn]}]
  (p/let [payload (js/JSON.stringify
                   (clj->js {:username username
                             :password password
                             :deviceLabel device-label}))
          {:keys [status body]} ((or fetch-fn default-fetch-json!)
                                  {:method "POST"
                                   :url (login-url base-url)
                                   :headers {"Content-Type" "application/json"}
                                   :body payload})
          parsed (parse-response-body body)]
    (if (<= 200 status 299)
      {:token (:token parsed) :session-id (:sessionId parsed)}
      (let [error (:error parsed)]
        (throw (ex-info (or (:message error) "login failed")
                        (cond-> {:status status
                                 :code (normalize-code (:code error))}
                          error (assoc :error error))))))))

(defn upload-asset!
  "PUTs raw asset bytes to a db-worker-node daemon's /v1/assets/:repo/:name
  (see Fase 5 in docs/agent-guide). Used by frontend.fs/write-asset-file! for
  a web/mobile client, which has no local filesystem to write assets to --
  Electron keeps writing straight to disk via its own IPC instead."
  [{:keys [base-url auth-token fetch-fn]} repo file-name data]
  (p/let [{:keys [status body]} ((or fetch-fn default-fetch-json!)
                                 {:method "PUT"
                                  :url (asset-url base-url repo file-name)
                                  :headers (binary-headers auth-token)
                                  :body data})]
    (if (<= 200 status 299)
      true
      (let [parsed (parse-response-body body)
            error (:error parsed)]
        (throw (ex-info (or (:message error) "asset upload failed")
                        (cond-> {:status status
                                 :code (normalize-code (:code error))}
                          error (assoc :error error))))))))

(defn- decode-event
  [{:keys [type payload]}]
  (let [decoded (when (some? payload)
                  (try
                    (ldb/read-transit-str payload)
                    (catch :default _
                      payload)))]
    (if (and (vector? decoded)
             (= 2 (count decoded))
             (keyword? (first decoded)))
      [(first decoded) (second decoded)]
      [(when type (keyword type)) decoded])))

(defn- data-line
  [event-text]
  (some (fn [line]
          (when (string/starts-with? line "data: ")
            (subs line 6)))
        (string/split-lines event-text)))

(defn- default-open-ws-fn
  [{:keys [url auth-token]}]
  (js/WebSocket. url (str "bearer." auth-token)))

(defn create-client
  [{:keys [fetch-fn open-sse-fn open-ws-fn schedule-fn reconnect-delay-ms] :as opts}]
  (let [default-fetch-fn
        (fn [{:keys [method url headers body]}]
          (p/let [^js res (js/fetch url (clj->js (cond-> {:method method
                                                          :headers (or headers {})}
                                                   body (assoc :body body))))
                  text (.text res)]
            {:status (.-status res)
             :body text}))
        default-open-sse-fn
        (fn [{:keys [url on-message on-error]}]
          (if (exists? js/EventSource)
            (let [es (js/EventSource. url)]
              (set! (.-onmessage es)
                    (fn [event]
                      (when on-message
                        (on-message (str "data: " (.-data event) "\n\n")))))
              (set! (.-onerror es)
                    (fn [event]
                      (when on-error
                        (on-error event))))
              {:close! (fn []
                         (.close es))})
            {:close! (fn [] nil)}))]
    (assoc opts
           :fetch-fn (or fetch-fn default-fetch-fn)
           :open-sse-fn (or open-sse-fn default-open-sse-fn)
           :open-ws-fn (or open-ws-fn default-open-ws-fn)
           :schedule-fn (or schedule-fn (fn [f delay-ms]
                                          (js/setTimeout f delay-ms)))
           :reconnect-delay-ms (or reconnect-delay-ms 1000))))

(defn invoke!
  [{:keys [base-url auth-token fetch-fn on-invoke-success on-invoke-failure]} method args]
  (let [payload (js/JSON.stringify
                 (clj->js {:method method
                           :argsTransit (ldb/write-transit-str args)}))]
    (->
     (p/let [{:keys [status body]}
             (fetch-fn {:method "POST"
                        :url (invoke-url base-url)
                        :headers (base-headers auth-token)
                        :body payload})
             parsed (parse-response-body body)]
       (if (<= 200 status 299)
         (let [result (ldb/read-transit-str (:resultTransit parsed))]
           (when on-invoke-success
             (on-invoke-success method args result))
           result)
         (let [error (:error parsed)]
           (throw (ex-info (or (:message error) "db-worker invoke failed")
                           (cond-> {:status status
                                    :code (normalize-code (:code error))}
                             error (assoc :error error)))))))
     (p/catch (fn [error]
                (when on-invoke-failure
                  (on-invoke-failure method args error))
                (throw error))))))

(defn create-invoke-batcher
  "Coalesces thread-api invokes issued within a short window into a single
  POST /v1/invoke-batch instead of one HTTP round trip per call -- the
  generalization, to every wrapped-worker call a remote client makes, of the
  request-group batching frontend.db.subs-loader already does for
  get-render-snapshots specifically (Fase 6). Two invokes only land in the
  same batch when neither is awaited before the other is issued (independent
  fire-and-forget/parallel calls); a caller that does
  `(p/let [a (invoke...) b (invoke...)] ...)` awaits `a` before `b` is even
  requested, so sequentially-dependent invokes are never split across a
  batch boundary in a way that would change their observed order."
  [{:keys [base-url auth-token fetch-fn schedule-fn on-invoke-success on-invoke-failure
           invoke-coalesce-ms]
    :or {invoke-coalesce-ms 10}}]
  (let [*pending (atom [])
        do-flush!
        (fn []
          (let [batch @*pending]
            (reset! *pending [])
            (when (seq batch)
              (let [payload (js/JSON.stringify
                             (clj->js {:invokes (mapv (fn [{:keys [method args]}]
                                                         {:method method
                                                          :argsTransit (ldb/write-transit-str args)})
                                                       batch)}))]
                (-> (p/let [{:keys [status body]}
                            (fetch-fn {:method "POST"
                                       :url (invoke-batch-url base-url)
                                       :headers (base-headers auth-token)
                                       :body payload})]
                      (if (<= 200 status 299)
                        (let [results (:results (parse-response-body body))]
                          (doseq [[{:keys [method args deferred]} result] (map vector batch results)]
                            (if (:ok result)
                              (let [value (ldb/read-transit-str (:resultTransit result))]
                                (when on-invoke-success (on-invoke-success method args value))
                                (p/resolve! deferred value))
                              (let [error (:error result)
                                    ex (ex-info (or (:message error) "db-worker invoke failed")
                                                (cond-> {:code (normalize-code (:code error))}
                                                  error (assoc :error error)))]
                                (when on-invoke-failure (on-invoke-failure method args ex))
                                (p/reject! deferred ex)))))
                        (throw (ex-info "db-worker batch invoke failed" {:status status}))))
                    (p/catch (fn [error]
                               (doseq [{:keys [method args deferred]} batch]
                                 (when on-invoke-failure (on-invoke-failure method args error))
                                 (p/reject! deferred error)))))))))]
    (fn invoke-batched! [method args]
      (let [deferred (p/deferred)
            schedule? (empty? @*pending)]
        (swap! *pending conj {:method method :args args :deferred deferred})
        (when schedule?
          (schedule-fn do-flush! invoke-coalesce-ms))
        deferred))))

(defn- import-db-binary!
  [{:keys [base-url auth-token fetch-fn on-invoke-success on-invoke-failure]} repo data]
  (let [method "thread-api/import-db-binary"
        args [repo data]]
    (->
     (p/let [{:keys [status body]}
             (fetch-fn {:method "POST"
                        :url (import-db-binary-url base-url repo)
                        :headers (binary-headers auth-token)
                        :body data})
             parsed (parse-response-body body)]
       (if (<= 200 status 299)
         (let [result (ldb/read-transit-str (:resultTransit parsed))]
           (when on-invoke-success
             (on-invoke-success method args result))
           result)
         (let [error (:error parsed)]
           (throw (ex-info (or (:message error) "db-worker invoke failed")
                           (cond-> {:status status
                                    :code (normalize-code (:code error))}
                             error (assoc :error error)))))))
     (p/catch (fn [error]
                (when on-invoke-failure
                  (on-invoke-failure method args error))
                (throw error))))))

(defn connect-events!
  [{:keys [base-url auth-token event-handler open-sse-fn schedule-fn reconnect-delay-ms on-event-error]} wrapped-worker]
  (let [connected? (atom true)
        buffer (atom "")
        subscription (atom nil)
        close-subscription! (fn []
                              (when-let [close! (:close! @subscription)]
                                (close!))
                              (reset! subscription nil))
        dispatch! (fn [event-str]
                    (when-let [line (data-line event-str)]
                      (let [event (parse-response-body line)
                            [event-type payload] (decode-event event)]
                        (when (and event-handler event-type)
                          (event-handler event-type wrapped-worker payload)))))]
    (letfn [(open! []
              (when @connected?
                (reset! subscription
                        (open-sse-fn
                         {:url (events-url base-url auth-token)
                          :headers (base-headers auth-token)
                          :on-message (fn [chunk]
                                        (when @connected?
                                          (swap! buffer str chunk)
                                          (loop []
                                            (when-let [idx (string/index-of @buffer "\n\n")]
                                              (let [event-str (subs @buffer 0 idx)
                                                    next-buffer (subs @buffer (+ idx 2))]
                                                (reset! buffer next-buffer)
                                                (dispatch! event-str)
                                                (recur))))))
                          :on-error (fn [error]
                                      (when @connected?
                                        (close-subscription!)
                                        (when on-event-error
                                          (on-event-error error))
                                        (when @connected?
                                          (schedule-fn open! reconnect-delay-ms))))}))))]
      (open!)
      {:disconnect! (fn []
                      (reset! connected? false)
                      (close-subscription!)
                      nil)})))

(defn connect-events-ws!
  "WebSocket transport for the daemon event channel -- the default since
  Fase 4 (connect-events! above stays fully wired as an instant rollback,
  see docs/agent-guide). Resumes by revision on every (re)connect: sends
  {type: subscribe, graphId: repo, sinceRev: N} where N is the renderer's
  last applied revision. The server replays buffered deltas gap-free, or --
  if the gap is wider than its ~500-entry buffer -- replies {type: resync},
  handled here as a normal event-handler dispatch so the existing
  frontend.handler.worker / :db/resync wiring can discard cached renderer
  state and reload."
  [{:keys [base-url auth-token repo event-handler open-ws-fn schedule-fn
           reconnect-delay-ms on-event-error current-rev-fn]}
   wrapped-worker]
  (let [connected? (atom true)
        socket (atom nil)
        current-rev-fn (or current-rev-fn db-subs/current-rev)
        close-socket! (fn []
                        (when-let [^js s @socket]
                          (set! (.-onopen s) nil)
                          (set! (.-onmessage s) nil)
                          (set! (.-onerror s) nil)
                          (set! (.-onclose s) nil)
                          (try (.close s) (catch :default _ nil)))
                        (reset! socket nil))
        dispatch-control! (fn [{:keys [type rev message]}]
                            (case type
                              "resync" (when event-handler
                                         (event-handler :resync wrapped-worker {:rev rev}))
                              "error" (log/error :db-worker-node-ws-server-error {:message message})
                              nil))
        dispatch-business! (fn [event]
                             (let [[event-type payload] (decode-event event)]
                               (when (and event-handler event-type)
                                 (event-handler event-type wrapped-worker payload))))]
    (letfn [(open! []
              (when @connected?
                (let [^js s (open-ws-fn {:url (events-ws-url base-url) :auth-token auth-token})]
                  (reset! socket s)
                  (set! (.-onopen s)
                        (fn [_]
                          (when @connected?
                            (.send s (js/JSON.stringify
                                      (clj->js {:type "subscribe"
                                                :graphId repo
                                                :sinceRev (current-rev-fn)}))))))
                  (set! (.-onmessage s)
                        (fn [event]
                          (when @connected?
                            (let [parsed (parse-response-body (.-data event))]
                              (if (contains? parsed :payload)
                                (dispatch-business! parsed)
                                (dispatch-control! parsed))))))
                  (set! (.-onerror s)
                        (fn [error]
                          (when (and @connected? on-event-error)
                            (on-event-error error))))
                  (set! (.-onclose s)
                        (fn [_]
                          (reset! socket nil)
                          (when @connected?
                            (schedule-fn open! reconnect-delay-ms)))))))]
      (open!)
      {:disconnect! (fn []
                      (reset! connected? false)
                      (close-socket!)
                      nil)})))

(defn- method->str
  [qkw]
  (str (namespace qkw) "/" (name qkw)))

(defrecord InRemote [client wrapped-worker disconnect!]
  protocol/PersistentDB
  (<new [_this repo opts]
    (invoke! client "thread-api/create-or-open-db" [repo opts]))

  (<list-db [_this]
    (invoke! client "thread-api/list-db" []))

  (<unsafe-delete [_this repo]
    (invoke! client "thread-api/unsafe-unlink-db" [repo]))

  (<release-access-handles [_this repo]
    (invoke! client "thread-api/release-access-handles" [repo]))

  (<open-and-fetch-schema [_this repo opts]
    (invoke! client "thread-api/create-or-open-db" [repo opts]))

  (<export-db [_this repo _opts]
    (invoke! client "thread-api/export-db-binary" [repo]))

  (<import-db [_this repo data]
    (->
     (import-db-binary! client repo data)
     (p/catch (fn [error]
                (log/error :import-db-error repo error "SQLiteDB import error")
                (notification/show! (t :storage/sqlitedb-import-error error) :error) {})))))

(defn start!
  [{:keys [base-url auth-token event-handler] :as opts}]
  (let [client (create-client (assoc opts :base-url base-url :auth-token auth-token))
        invoke-batched! (create-invoke-batcher client)
        wrapped-worker (fn [qkw & args]
                         (invoke-batched! (method->str qkw) args))
        {:keys [disconnect!]} (connect-events-ws! (assoc client
                                                         :event-handler event-handler
                                                         :auth-token auth-token)
                                                  wrapped-worker)]
    (->InRemote client wrapped-worker disconnect!)))

(defn stop!
  [remote-client]
  (when-let [disconnect! (:disconnect! remote-client)]
    (disconnect!))
  (p/resolved true))
