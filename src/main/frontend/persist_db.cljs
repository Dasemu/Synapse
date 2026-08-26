(ns frontend.persist-db
  "Backend of DB based graph"
  (:require [electron.ipc :as ipc]
            [frontend.config :as config]
            [frontend.context.i18n :refer [t]]
            [frontend.db.transact :as db-transact]
            [frontend.handler.notification :as notification]
            [frontend.handler.worker :as worker-handler]
            [frontend.persist-db.browser :as browser]
            [frontend.persist-db.protocol :as protocol]
            [frontend.persist-db.remote :as remote]
            [frontend.rfx :as rfx]
            [frontend.state :as state]
            [frontend.util :as util]
            [frontend.util.text :as text-util]
            [lambdaisland.glogi :as log]
            [logseq.common.graph-dir :as graph-dir]
            [logseq.db :as ldb]
            [promesa.core :as p]))

(def db-worker-recovery-failure-threshold 1)

(defonce opfs-db (browser/->InBrowser))
(defonce remote-db (atom nil))
(defonce remote-repo (atom nil))
(defonce remote-runtime-state (atom nil))

(declare <ensure-remote! <release-remote-runtime-slot!)

(defn- clear-remote-runtime!
  []
  (reset! remote-runtime-state nil)
  (reset! remote-db nil)
  (reset! remote-repo nil)
  (reset! state/*db-worker nil))

(defn- same-remote-repo?
  [repo runtime-repo]
  (graph-dir/same-repo? repo runtime-repo))

(defn- <stop-remote-if-current!
  [repo]
  (if (and repo (same-remote-repo? repo @remote-repo))
    (if-let [remote-client @remote-db]
      (-> (remote/stop! remote-client)
          (p/finally
            (fn []
              (when (same-remote-repo? repo @remote-repo)
                (clear-remote-runtime!)))))
      (do
        (clear-remote-runtime!)
        (p/resolved true)))
    (p/resolved false)))

(defn- set-remote-runtime!
  [repo client session-id]
  (reset! remote-runtime-state {:repo repo
                                :client client
                                :session-id session-id
                                :request-failures 0
                                :recovery-triggered? false})
  (reset! remote-db client)
  (reset! remote-repo repo)
  (reset! state/*db-worker (:wrapped-worker client)))

(defn- active-runtime-session?
  [state repo session-id]
  (and (same-remote-repo? repo (:repo state))
       (= session-id (:session-id state))))

(defn- active-runtime-client?
  [state repo session-id client]
  (and (active-runtime-session? state repo session-id)
       (identical? client (:client state))))

(defn- reset-active-request-failures!
  [repo session-id]
  (swap! remote-runtime-state
         (fn [state]
           (if (active-runtime-session? state repo session-id)
             (assoc state :request-failures 0)
             state))))

(defn- server-unavailable-error?
  [error]
  (let [{:keys [status code]} (ex-data error)]
    (or (nil? (ex-data error))
        (= :server-unavailable code)
        (= :db-worker-unavailable code)
        (= :connection-refused code)
        (= :fetch-failed code)
        (= :network-error code)
        (= 0 status))))

(defn- event-stream-error-loggable?
  [failure-count]
  (and (pos-int? failure-count)
       (or (and (<= failure-count 64)
                (zero? (bit-and failure-count (dec failure-count))))
           (and (> failure-count 64)
                (zero? (mod failure-count 64))))))

(defn- <release-active-runtime!
  [repo remote-client session-id]
  (if (active-runtime-client? @remote-runtime-state repo session-id remote-client)
    (p/let [_ (do
                (clear-remote-runtime!)
                (<release-remote-runtime-slot! repo))]
      true)
    (do
      (log/info :event :db-worker-runtime-recovery-skipped
                :repo repo
                :reason :runtime-changed)
      (p/resolved false))))

(defn- <trigger-db-worker-runtime-recovery!
  [repo remote-client session-id]
  (log/warn :event :db-worker-runtime-recovering :repo repo)
  (-> (p/do!
       (when remote-client
         (-> (remote/stop! remote-client)
             (p/catch (fn [error]
                        (log/warn :event :db-worker-runtime-stop-error
                                  :repo repo
                                  :error error))))))
      (p/then (fn [_]
                (<release-active-runtime! repo remote-client session-id)))
      (p/then (fn [released?]
                (if (and released?
                         (same-remote-repo? repo (state/get-current-repo)))
                  (<ensure-remote! repo {:only-if-current? true})
                  (when-not released?
                    (log/info :event :db-worker-runtime-recovery-skipped
                              :repo repo
                              :reason :release-skipped)))))
      (p/then (fn [client]
                (when client
                  (log/info :event :db-worker-runtime-recovered :repo repo))))
      (p/catch (fn [error]
                 (log/error :event :db-worker-runtime-recovery-failed
                            :repo repo
                            :error error)
                 (notification/show!
                  (t :graph/db-worker-recovery-failed-error
                     (text-util/get-graph-name-from-path repo))
                  :error)))))

(defn- record-active-request-failure!
  [repo session-id error]
  (when (server-unavailable-error? error)
    (let [triggered? (atom false)
          remote-client (atom nil)]
      (swap! remote-runtime-state
             (fn [state]
               (if (and (active-runtime-session? state repo session-id)
                        (not (:recovery-triggered? state)))
                 (let [failures (inc (or (:request-failures state) 0))]
                   (if (>= failures db-worker-recovery-failure-threshold)
                     (do
                       (reset! triggered? true)
                       (reset! remote-client (:client state))
                       (assoc state
                              :request-failures failures
                              :recovery-triggered? true))
                     (assoc state :request-failures failures)))
                 state)))
      (when @triggered?
        (<trigger-db-worker-runtime-recovery! repo @remote-client session-id))
      nil)))

(defn- node-runtime?
  []
  (and (exists? js/process)
       (not (exists? js/window))))

(defn- electron-runtime?
  []
  (and (not (node-runtime?))
       (util/electron?)))

(defn- web-remote-configured?
  "True when running as a non-Electron, non-Node client (browser or
   Capacitor) with a db-worker-node server configured -- the counterpart to
   Electron's IPC-managed local runtime, driven by config/get-server-base-url
   instead."
  []
  (and (not (node-runtime?))
       (not (util/electron?))
       (some? (config/get-server-base-url))))

(defn- remote-runtime?
  "True whenever this client talks to a remote db-worker-node daemon over
   HTTP/SSE rather than an in-process worker (frontend.persist-db.browser).
   Replaces the old electron-runtime? checks at every PersistentDB dispatch
   site below, so the same remote path serves Electron and a configured web
   or mobile client alike."
  []
  (or (electron-runtime?)
      (web-remote-configured?)))

(defn- <resolve-remote-runtime!
  "Resolves {:base-url :auth-token} for the active remote transport.
   Electron gets it from the main process, which owns spawning/reusing the
   local daemon (see logseq.cli.server/ensure-server!); a web/mobile client
   reads its own locally configured server instead -- there is no daemon to
   spawn, only one to log into."
  [repo]
  (if (electron-runtime?)
    (ipc/ipc "db-worker-runtime" repo)
    (p/resolved {:base-url (config/get-server-base-url)
                 :auth-token (config/get-server-token)})))

(defn- <release-remote-runtime-slot!
  "Releases the Electron-owned runtime slot for repo, if any. No-op for a
   web/mobile client: there's no per-window runtime manager on the other
   end to release, just the daemon's own session, which the client simply
   stops using."
  [repo]
  (if (electron-runtime?)
    (ipc/ipc "releaseDbWorkerRuntime" repo)
    (p/resolved nil)))

(defn- <sync-markdown-mirror-setting!
  [repo]
  (state/<invoke-db-worker :thread-api/markdown-mirror-set-enabled
                           repo
                           (true? (:feature/markdown-mirror? (state/get-graph-config repo)))))

(defn- graph-markdown-mirror-enabled?
  [state repo]
  (true? (get-in state [:config repo :feature/markdown-mirror?])))

(defn- sync-markdown-mirror-setting-watch!
  []
  (rfx/unlisten! :sync-markdown-mirror-setting)
  (rfx/listen!
   :sync-markdown-mirror-setting
   (fn [old-state new-state]
     (let [repo (:git/current-repo new-state)
           old-enabled? (graph-markdown-mirror-enabled? old-state repo)
           new-enabled? (graph-markdown-mirror-enabled? new-state repo)]
       (when (and repo
                  @state/*db-worker
                  (not= old-enabled? new-enabled?))
         (-> (state/<invoke-db-worker :thread-api/markdown-mirror-set-enabled
                                      repo
                                      new-enabled?)
             (p/catch (fn [error]
                        (log/error :markdown-mirror/settings-watch-sync-failed
                                   {:repo repo
                                    :enabled? new-enabled?
                                    :error error}))))))))
  nil)

(defn- <ensure-remote!
  ([repo] (<ensure-remote! repo nil))
  ([repo {:keys [only-if-current?]}]
   (let [current-for-repo? #(same-remote-repo? repo (state/get-current-repo))]
     (cond
       (nil? repo)
       (p/resolved @remote-db)

       (and only-if-current? (not (current-for-repo?)))
       (do
         (log/warn :event :db-worker-ensure-remote-stale
                   :repo repo :phase :before-stop)
         (p/resolved nil))

       (same-remote-repo? repo @remote-repo)
       (p/resolved @remote-db)

       :else
       (let [session-id (str (random-uuid))
             event-stream-failures (atom 0)]
         (p/let [_ (when @remote-db
                     (remote/stop! @remote-db))]
           (if (and only-if-current? (not (current-for-repo?)))
             (do
               (log/warn :event :db-worker-ensure-remote-stale
                         :repo repo :phase :before-runtime)
               nil)
             (p/let [runtime (<resolve-remote-runtime! repo)
                     client (remote/start! (assoc runtime
                                                  :repo repo
                                                  :event-handler worker-handler/handle
                                                  :on-invoke-success (fn [_method _args _result]
                                                                       (reset-active-request-failures! repo session-id))
                                                  :on-invoke-failure (fn [_method _args error]
                                                                       (record-active-request-failure! repo session-id error))
                                                  :on-event-error (fn [error]
                                                                    (let [failure-count (swap! event-stream-failures inc)]
                                                                      (when (event-stream-error-loggable? failure-count)
                                                                        (log/warn :event :db-worker-event-stream-error
                                                                                  :repo repo
                                                                                  :failures failure-count
                                                                                  :error error))
                                                                      (record-active-request-failure!
                                                                       repo
                                                                       session-id
                                                                       (ex-info "db-worker event stream unavailable"
                                                                                {:code :db-worker-unavailable
                                                                                 :event-stream? true
                                                                                 :cause error}))))))]
               (if (and only-if-current? (not (current-for-repo?)))
                 (do
                   (log/warn :event :db-worker-ensure-remote-stale
                             :repo repo :phase :after-start)
                   (-> (remote/stop! client)
                       (p/catch (fn [e]
                                  (log/warn :event :db-worker-stale-client-stop-error
                                            :repo repo :error e)))
                       (p/then (fn [_]
                                 (p/let [_ (if (same-remote-repo? repo @remote-repo)
                                             (log/info :event :db-worker-stale-release-skipped
                                                       :repo repo
                                                       :reason :runtime-changed)
                                             (<release-remote-runtime-slot! repo))]
                                   nil)))
                       (p/catch (fn [e]
                                  (log/warn :event :db-worker-stale-release-error
                                            :repo repo :error e)))))
                 (do
                   (set-remote-runtime! repo client session-id)
                   (p/let [_ (<sync-markdown-mirror-setting! repo)]
                     (sync-markdown-mirror-setting-watch!)
                     nil)
                   (ldb/register-transact-fn!
                    (fn remote-transact!
                      [repo tx-data tx-meta]
                      (db-transact/transact browser/transact!
                                            (if (string? repo) repo (state/get-current-repo))
                                            tx-data
                                            (assoc tx-meta :client-id (:client-id (state/get-state))))))
                   client))))))))))

(defn <start-runtime!
  []
  (cond
    (remote-runtime?)
    (if-let [repo (state/get-current-repo)]
      (<ensure-remote! repo)
      (p/resolved nil))

    :else
    (browser/start-db-worker!)))

(defn- get-impl
  "Get the actual implementation of PersistentDB"
  []
  opfs-db)

(defn <list-db []
  (if (remote-runtime?)
    (if-let [repo (or @remote-repo (state/get-current-repo))]
      (p/let [client (<ensure-remote! repo)]
        (protocol/<list-db client))
      (p/resolved []))
    (protocol/<list-db (get-impl))))

(defn <unsafe-delete [repo]
  (when repo
    (if (remote-runtime?)
      (p/let [client (<ensure-remote! repo)]
        (protocol/<unsafe-delete client repo))
      (protocol/<unsafe-delete (get-impl) repo))))

(defn <close-db [repo]
  (when repo
    (if (remote-runtime?)
      (if (same-remote-repo? repo @remote-repo)
        (if-let [remote-client @remote-db]
          (p/let [_ (-> (remote/invoke! (:client remote-client) "thread-api/close-db" [repo])
                        (p/catch (fn [_] nil)))
                  _ (<stop-remote-if-current! repo)]
            nil)
          (p/resolved nil))
        (p/resolved nil))
      (state/<invoke-db-worker :thread-api/close-db repo))))

(defn <export-db
  [repo opts]
  (when repo
    (if (remote-runtime?)
      (p/let [client (<ensure-remote! repo)]
        (protocol/<export-db client repo opts))
      (protocol/<export-db (get-impl) repo opts))))

(defn <import-db
  [repo data]
  (when repo
    (if (remote-runtime?)
      (p/let [client (<ensure-remote! repo)]
        (protocol/<import-db client repo data))
      (protocol/<import-db (get-impl) repo data))))

(defn <open-and-fetch-schema
  ([repo]
   (<open-and-fetch-schema repo {}))
  ([repo opts]
   (when repo
     (if (remote-runtime?)
       (p/let [client (<ensure-remote! repo)]
         (protocol/<open-and-fetch-schema client repo opts))
       (protocol/<open-and-fetch-schema (get-impl) repo opts)))))

;; FIXME: limit repo name's length and sanity
;; @shuyu Do we still need this?
(defn <new [repo opts]
  {:pre [(<= (count repo) 128)]}
  (p/let [impl (if (remote-runtime?)
                 (<ensure-remote! repo)
                 (p/resolved (get-impl)))
          _ (protocol/<new impl repo opts)]
    (<export-db repo {})))

(defn export-current-graph!
  [& {:keys [succ-notification?]}]
  (when (util/electron?)
    (when-let [repo (state/get-current-repo)]
      (log/debug :event :backup-db :graph repo)
      (->
       (p/do!
        (ipc/ipc :db-export repo true)
        (when succ-notification?
          (state/pub-event!
           [:notification/show {:content "DB backup successfully."
                                :status :success}])))
       (p/catch (fn [^js error]
                  (log/error :event :db-backup-failed :graph repo :error error)
                  (state/pub-event!
                   [:notification/show {:content (str (.getMessage error))
                                        :status :error
                                        :clear? false}])))))))
