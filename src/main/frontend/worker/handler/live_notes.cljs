(ns frontend.worker.handler.live-notes
  "Live notes: pages tagged :logseq.class/LiveNote whose content is fetched
   from an external URL on a refresh schedule.

  Native, not a plugin: the plugin API's fenced-code-renderer + exper_request
  pattern only runs in Electron and has no refresh scheduler or secure
  credential store. This follows the render-resource guidance instead --
  a background poller `transact!`s fetched content into the graph as a
  normal property, and the existing render-resource/affected-keys pipeline
  (see render_resource/live_note.cljs) picks up the change and pushes it to
  clients with zero bespoke plumbing on the read side.

  Node-only, like search embeddings and CalDAV: outbound fetch and the
  platform keychain only exist in the daemon runtime."
  (:require [datascript.core :as d]
            [frontend.common.thread-api :refer [def-thread-api]]
            [frontend.worker.platform :as platform]
            [frontend.worker.state :as worker-state]
            [lambdaisland.glogi :as log]
            [logseq.db :as ldb]
            [promesa.core :as p]))

(def ^:private default-refresh-minutes 60)
(def ^:private min-refresh-minutes 5)
(def ^:private max-content-length 8000)
(def ^:private tick-interval-ms (* 5 60 1000))

(defn- node-runtime?
  []
  (= :node (platform/env-flag (platform/current) :runtime)))

(defn- auth-secret-key
  [repo block-uuid]
  (str "live-note:" repo ":" block-uuid))

(defn <save-auth!
  [repo block-uuid auth-header]
  (platform/save-secret-text! (platform/current) (auth-secret-key repo block-uuid) auth-header))

(defn <read-auth
  [repo block-uuid]
  (platform/read-secret-text (platform/current) (auth-secret-key repo block-uuid)))

(defn <delete-auth!
  [repo block-uuid]
  (platform/delete-secret-text! (platform/current) (auth-secret-key repo block-uuid)))

(defn effective-refresh-minutes
  [refresh-minutes]
  (max min-refresh-minutes (or refresh-minutes default-refresh-minutes)))

(defn due?
  "Whether a live note is due for a refresh: never fetched, or its own
   refresh interval has elapsed since the last fetch."
  [now-ms last-fetched-at refresh-minutes]
  (or (not (number? last-fetched-at))
      (>= (- now-ms last-fetched-at)
          (* (effective-refresh-minutes refresh-minutes) 60 1000))))

(defn truncate-content
  [text]
  (when (string? text)
    (if (> (count text) max-content-length)
      (str (subs text 0 max-content-length) "\n...(truncated)")
      text)))

(defn- <fetch-content!
  [fetch-fn url auth-header]
  (-> (fetch-fn url (clj->js (cond-> {:method "GET"}
                              auth-header (assoc :headers {"Authorization" auth-header}))))
      (p/then (fn [resp]
                (if (.-ok resp)
                  (.text resp)
                  (p/rejected (ex-info (str "HTTP " (.-status resp)) {:status (.-status resp)})))))))

(defn- <refresh-page!
  [conn fetch-fn repo page]
  (let [url (:logseq.property.livenote/url page)]
    (-> (p/let [auth-header (<read-auth repo (:block/uuid page))
                text (<fetch-content! fetch-fn url auth-header)]
          (ldb/transact! conn
                        [[:db/add (:db/id page) :logseq.property.livenote/content (truncate-content text)]
                         [:db/add (:db/id page) :logseq.property.livenote/last-fetched-at (js/Date.now())]
                         [:db/retract (:db/id page) :logseq.property.livenote/last-error]]))
        (p/catch (fn [error]
                   (log/warn :live-notes/refresh-failed
                            {:page-uuid (str (:block/uuid page)) :error (str error)})
                   (ldb/transact! conn
                                 [[:db/add (:db/id page) :logseq.property.livenote/last-fetched-at (js/Date.now())]
                                  [:db/add (:db/id page) :logseq.property.livenote/last-error (str error)]]))))))

(defn- live-note-pages
  "Guarded on the class existing: an empty graph (skips built-in initial
   data) or a graph created before this migration step has no
   :logseq.class/LiveNote ident, and an :avet lookup on a non-existent ident
   throws rather than returning no results."
  [db]
  (when (d/entity db :logseq.class/LiveNote)
    (->> (d/datoms db :avet :block/tags :logseq.class/LiveNote)
         (map (fn [datom] (d/entity db (:e datom))))
         (remove #(or (:logseq.property/deleted-at %) (ldb/hidden? %)))
         (filter :logseq.property.livenote/url))))

(defn- due-pages
  [now-ms pages]
  (filter (fn [page]
            (due? now-ms
                  (:logseq.property.livenote/last-fetched-at page)
                  (:logseq.property.livenote/refresh-minutes page)))
          pages))

(defn <tick!
  ([repo] (<tick! repo js/fetch))
  ([repo fetch-fn]
   (if-let [conn (worker-state/get-datascript-conn repo)]
     (let [due (due-pages (js/Date.now()) (live-note-pages @conn))]
       (p/all (map #(<refresh-page! conn fetch-fn repo %) due)))
     (p/resolved []))))

(defonce ^:private *intervals (atom {}))

(defn stop-refresh!
  [repo]
  (when-let [interval-id (get @*intervals repo)]
    (js/clearInterval interval-id))
  (swap! *intervals dissoc repo))

(defn maybe-start-refresh!
  [repo]
  (when (node-runtime?)
    (stop-refresh! repo)
    (<tick! repo)
    (swap! *intervals assoc repo (js/setInterval #(<tick! repo) tick-interval-ms))))

(def-thread-api :thread-api/live-note-refresh-now!
  [repo block-uuid]
  (if-let [conn (worker-state/get-datascript-conn repo)]
    (if-let [page (d/entity @conn [:block/uuid block-uuid])]
      (p/let [_ (<refresh-page! conn js/fetch repo page)]
        {:status :ok})
      (p/resolved {:status :not-found}))
    (p/resolved {:status :not-found})))

(def-thread-api :thread-api/live-note-set-auth!
  [repo block-uuid auth-header]
  (p/let [_ (if (seq auth-header)
             (<save-auth! repo block-uuid auth-header)
             (<delete-auth! repo block-uuid))]
    {:status :ok}))
