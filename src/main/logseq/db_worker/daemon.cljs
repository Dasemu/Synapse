(ns logseq.db-worker.daemon
  "Shared db-worker-node lifecycle helpers for CLI and Electron."
  (:require ["child_process" :as child-process]
            ["crypto" :as crypto]
            ["fs" :as fs]
            ["http" :as http]
            ["path" :as node-path]
            [clojure.string :as string]
            [lambdaisland.glogi :as log]
            [logseq.db-worker.log :as db-worker-log]
            [promesa.core :as p]))

(def ^:private valid-owner-sources
  #{:cli :electron :unknown})

(def ^:private embedding-url-env "LOGSEQ_EMBEDDINGS_URL")

(defn normalize-owner-source
  [owner-source]
  (let [owner-source (cond
                       (keyword? owner-source) owner-source
                       (string? owner-source) (keyword (string/trim owner-source))
                       :else :unknown)]
    (if (contains? valid-owner-sources owner-source)
      owner-source
      :unknown)))

(defn pid-status
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

(defn read-lock
  [path]
  (when (and (seq path) (fs/existsSync path))
    (let [lock (js->clj (js/JSON.parse (.toString (fs/readFileSync path) "utf8"))
                        :keywordize-keys true)]
      (assoc lock :owner-source (normalize-owner-source (:owner-source lock))))))

(defn remove-lock!
  [path]
  (when (and (seq path) (fs/existsSync path))
    (fs/unlinkSync path)))

(defn http-request
  [{:keys [method host port path headers body timeout-ms]}]
  (p/create
   (fn [resolve reject]
     (let [timeout-ms (or timeout-ms 5000)
           start-ms (js/Date.now)
           req (.request
                http
                #js {:method method
                     :hostname host
                     :port port
                     :path path
                     :headers (clj->js (or headers {}))}
                (fn [^js res]
                  (let [chunks (array)]
                    (.on res "data" (fn [chunk] (.push chunks chunk)))
                    (.on res "end" (fn []
                                     (let [buf (js/Buffer.concat chunks)]
                                       (resolve {:status (.-statusCode res)
                                                 :body (.toString buf "utf8")
                                                 :elapsed-ms (- (js/Date.now) start-ms)}))))
                    (.on res "error" reject))))
           timeout-id (js/setTimeout
                       (fn []
                         (.destroy req)
                         (reject (ex-info "request timeout" {:code :timeout})))
                       timeout-ms)]
       (.on req "error" (fn [err]
                          (js/clearTimeout timeout-id)
                          (reject err)))
       (when body
         (.write req body))
       (.end req)
       (.on req "response" (fn [_]
                             (js/clearTimeout timeout-id)))))))

(defn ready?
  [{:keys [host port]}]
  (-> (p/let [{:keys [status]} (http-request {:method "GET"
                                              :host host
                                              :port port
                                              :path "/healthz"
                                              :timeout-ms 1000})]
        (= 200 status))
      (p/catch (fn [_] false))))

(defn healthy?
  [{:keys [host port]}]
  (-> (p/let [{:keys [status]} (http-request {:method "GET"
                                              :host host
                                              :port port
                                              :path "/healthz"
                                              :timeout-ms 1000})]
        (contains? #{200 503} status))
      (p/catch (fn [_] false))))

(defn- local-credential-path
  [root-dir]
  (node-path/join root-dir "db-worker-local-credential.json"))

(defn ensure-local-credential!
  "A local automated device credential for the CLI/Electron process that
  spawns and talks to its own db-worker-node daemon (see spawn-server!,
  which bootstraps the daemon's stored credential from this same
  username/password). Generated once and reused across restarts so the
  spawning process can log in without interactive input.

  Lives in root-dir alongside the daemon's other discovery/lock files:
  anyone with filesystem access to root-dir already has full read/write
  access to the graph's SQLite files, so a plaintext bootstrap password
  here doesn't widen the local trust boundary. It only bootstraps HTTP
  session auth, which exists to stop *network* clients, not local ones."
  [root-dir]
  (let [path (local-credential-path root-dir)]
    (if (fs/existsSync path)
      (js->clj (js/JSON.parse (.toString (fs/readFileSync path) "utf8")) :keywordize-keys true)
      (let [credential {:username "local"
                        :password (.toString (crypto/randomBytes 24) "base64url")}]
        (when-not (fs/existsSync root-dir)
          (fs/mkdirSync root-dir #js {:recursive true}))
        (fs/writeFileSync path (js/JSON.stringify (clj->js credential)))
        (try (fs/chmodSync path 384) (catch :default _ nil)) ;; 384 = 0o600, owner read/write only
        credential))))

(defn login!
  "Logs into a db-worker-node daemon's HTTP auth using the local device
  credential for root-dir, returning a bearer token. Always succeeds
  against a daemon this process itself spawned, since spawn-server! seeds
  that daemon's stored credential from the same local file on first run."
  [{:keys [host port root-dir device-label]}]
  (p/let [{:keys [username password]} (ensure-local-credential! root-dir)
          request-body (js/JSON.stringify (clj->js {:username username
                                                     :password password
                                                     :deviceLabel (or device-label "local")}))
          {:keys [status body]} (http-request {:method "POST"
                                               :host host
                                               :port port
                                               :path "/v1/auth/login"
                                               :headers {"Content-Type" "application/json"}
                                               :body request-body
                                               :timeout-ms 5000})]
    (if (= 200 status)
      (:token (js->clj (js/JSON.parse body) :keywordize-keys true))
      (throw (ex-info "db-worker-node login failed" {:code :login-failed :status status})))))

(defn valid-lock?
  [lock]
  (and (seq (:repo lock))
       (number? (:pid lock))
       (seq (:lock-id lock))))

(declare wait-for)

(defn- process-stopped?
  [pid]
  (not (contains? #{:alive :no-permission} (pid-status pid))))

(defn- terminate-process!
  [pid force?]
  (if (= "win32" (.-platform js/process))
    (let [args (cond-> ["/PID" (str pid) "/T"]
                 force? (conj "/F"))]
      (try
        (let [result (.spawnSync child-process "taskkill" (clj->js args)
                                 #js {:windowsHide true
                                      :stdio "ignore"})]
          (if (or (= 0 (.-status result))
                  (process-stopped? pid))
            (p/resolved nil)
            (p/rejected (ex-info "taskkill failed"
                                 {:code :taskkill-failed
                                  :pid pid
                                  :force? force?}))))
        (catch :default e
          (p/rejected e))))
    (try
      (.kill js/process pid (if force? "SIGKILL" "SIGTERM"))
      (p/resolved nil)
      (catch :default e
        (p/rejected e)))))

(defn- wait-for-process-stop!
  [pid timeout-ms]
  (wait-for (fn [] (p/resolved (process-stopped? pid)))
            {:timeout-ms timeout-ms
             :interval-ms 100}))

(defn- stop-stale-process!
  [{:keys [pid]}]
  (cond
    (not (number? pid))
    (p/resolved nil)

    (= pid (.-pid js/process))
    (p/resolved nil)

    (process-stopped? pid)
    (p/resolved nil)

    :else
    (-> (p/do!
         (-> (terminate-process! pid false)
             (p/catch (fn [e]
                        (log/warn :db-worker-daemon/stale-process-stop-failed
                                  {:pid pid :force? false :error e})
                        (p/resolved nil))))
         (wait-for-process-stop! pid 5000))
        (p/catch (fn [_]
                   (-> (terminate-process! pid true)
                       (p/catch (fn [e]
                                  (log/warn :db-worker-daemon/stale-process-stop-failed
                                            {:pid pid :force? true :error e})
                                  (p/resolved nil))))
                   (-> (wait-for-process-stop! pid 1000)
                       (p/catch (fn [_] nil))))))))

(defn cleanup-stale-lock!
  [path lock]
  (cond
    (nil? lock)
    (p/resolved nil)

    (= :not-found (pid-status (:pid lock)))
    (do
      (remove-lock! path)
      (p/resolved nil))

    (not (valid-lock? lock))
    (-> (stop-stale-process! lock)
        (p/then (fn [_]
                  (remove-lock! path)
                  nil)))

    :else
    (p/resolved nil)))

(defn wait-for
  [pred-fn {:keys [timeout-ms interval-ms]
            :or {timeout-ms 8000
                 interval-ms 50}}]
  (p/create
   (fn [resolve reject]
     (let [start (js/Date.now)
           tick (fn tick []
                  (p/let [ok? (pred-fn)]
                    (if ok?
                      (resolve true)
                      (if (> (- (js/Date.now) start) timeout-ms)
                        (reject (ex-info "timeout" {:code :timeout}))
                        (js/setTimeout tick interval-ms)))))]
       (tick)))))

(defn wait-for-lock
  [path]
  (wait-for (fn []
              (p/resolved (fs/existsSync path)))
            {:timeout-ms 8000
             :interval-ms 50}))

(defn wait-for-ready
  [lock]
  (wait-for (fn [] (ready? lock))
            {:timeout-ms 8000
             :interval-ms 50}))

(defn spawn-server!
  [{:keys [script repo root-dir owner-source create-empty-db? embedding-endpoint embedding-model-id]}]
  (let [owner-source (normalize-owner-source owner-source)
        detached? (not= owner-source :electron)
        {:keys [username password]} (ensure-local-credential! root-dir)
        args (clj->js (cond-> [script "--repo" repo "--root-dir" root-dir "--owner-source" (name owner-source)]
                        create-empty-db? (conj "--create-empty-db")
                        (seq embedding-endpoint) (conj "--embedding-endpoint" embedding-endpoint)
                        (seq embedding-model-id) (conj "--embedding-model-id" embedding-model-id)))
        env (js/Object.assign #js {} (.-env js/process)
                              #js {:ELECTRON_RUN_AS_NODE "1"
                                   :LOGSEQ_DAEMON_USERNAME username
                                   :LOGSEQ_DAEMON_PASSWORD password})]
    (when (and (= :electron owner-source)
               (not (seq embedding-endpoint)))
      (js-delete env embedding-url-env))
    (if-not script
      (do
        (log/warn :db-worker-daemon/missing-script {:repo repo :root-dir root-dir})
        nil)
      (let [stdio-config (when detached?
                           (db-worker-log/child-stdio! {:root-dir root-dir
                                                        :repo repo}))
            _ (when detached?
                (aset env db-worker-log/stdio-redirected-env "1"))
            child (try
                    (.spawn child-process (.-execPath js/process) args #js {:detached detached?
                                                                            :stdio (if detached?
                                                                                     (:stdio stdio-config)
                                                                                     "inherit")
                                                                            :env env})
                    (finally
                      (when-let [close! (:close! stdio-config)]
                        (close!))))]
        (when detached?
          (.unref child))
        child))))
