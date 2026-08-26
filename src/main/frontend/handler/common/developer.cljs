(ns frontend.handler.common.developer
  "Common fns for developer related functionality"
  (:require ["/frontend/utils" :as utils]
            [cljs.pprint :as pprint]
            [clojure.string :as string]
            [frontend.context.i18n :refer [t]]
            [frontend.format.mldoc :as mldoc]
            [frontend.handler.notification :as notification]
            [frontend.persist-db :as persist-db]
            [frontend.state :as state]
            [frontend.util :as util]
            [frontend.util.page :as page-util]
            [logseq.shui.ui :as shui]
            [promesa.core :as p]))

(def ^:private <invoke-db-worker state/<invoke-db-worker)

;; Fns used between menus and commands
(defn- debug-pull-result
  [result]
  (cond-> result
    (seq (:block/properties result))
    (assoc :block.debug/properties (into {} (:block/properties result)))

    (seq (:block/refs result))
    (assoc :block.debug/refs (mapv #(or (:block/title %) %) (:block/refs result)))))

(defn show-entity-data
  [eid]
  (p/let [result* (<invoke-db-worker :thread-api/pull
                                     (state/get-current-repo)
                                     '[*]
                                     eid)
          result (debug-pull-result result*)
          pull-data (with-out-str (pprint/pprint result))]
    (println pull-data)
    (notification/show!
     [:div.ls-wrap-widen
      [:pre.code (str "ID: " (:db/id result) "\n"
                      pull-data)]
      [:br]
      (shui/button {:size :sm
                    :on-click #(.writeText js/navigator.clipboard pull-data)}
                   (t :ui/copy-to-clipboard))]
     :success
     false)))

(defn show-content-ast
  [content format]
  (let [ast-data (-> (mldoc/->edn content format)
                     pprint/pprint
                     with-out-str)]
    (println ast-data)
    (notification/show!
     [:div.ls-wrap-widen
      ;; Show clipboard at top since content is really long for pages
      (shui/button {:size :sm
                    :on-click #(.writeText js/navigator.clipboard ast-data)}
                   (t :ui/copy-to-clipboard))
      [:br]
      [:pre.code ast-data]]
     :success
     false)))

;; Public Commands
(defn ^:export show-block-data []
  ;; Use editor state to locate most recent block
  (if-let [block-uuid (:block-id (first (state/get-editor-args)))]
    (show-entity-data [:block/uuid block-uuid])
    (notification/show! (t :block/not-found-warning) :warning)))

(defn ^:export show-block-ast []
  (if-let [{:block/keys [title format]} (:block (first (state/get-editor-args)))]
    (show-content-ast title (or format :markdown))
    (notification/show! (t :block/not-found-warning) :warning)))

(defn ^:export show-page-data []
  (if-let [page-id (page-util/get-current-page-id)]
    (show-entity-data page-id)
    (notification/show! (t :page/not-found-warning) :warning)))

(defn ^:export validate-db []
  (state/<invoke-db-worker :thread-api/validate-db (state/get-current-repo)))

(defn- client-ops-export-file-name
  [repo]
  (-> (or repo "graph")
      (string/replace #"^/+" "")
      (string/replace #"[\\/]+" "_")
      (str "_client_ops_" (quot (util/time-ms) 1000))))

(defn ^:export export-client-ops-sqlite
  []
  (if (and util/web-platform? (not (util/electron?)))
    (if-let [repo (state/get-current-repo)]
      (-> (state/<invoke-db-worker :thread-api/export-client-ops-db-binary repo)
          (p/then (fn [data]
                    (if (instance? js/Uint8Array data)
                      (let [filename (client-ops-export-file-name repo)
                            blob (js/Blob. #js [data] (clj->js {:type "application/octet-stream"}))]
                        (utils/saveToFile blob filename "sqlite")
                        (notification/show!
                         (t :graph.diagnostics/client-ops-export-success filename)
                         :success
                         false))
                      (notification/show!
                       (t :graph.diagnostics/client-ops-export-invalid-payload-warning
                          (pr-str (type data)))
                       :warning))))
        (p/catch (fn [error]
                   (js/console.error "export-client-ops-sqlite failed:" error)
                   (notification/show! (t :graph.diagnostics/client-ops-export-failed-error) :error))))
      (notification/show! (t :graph.diagnostics/no-graph-warning) :warning))
    (notification/show! (t :graph.diagnostics/client-ops-export-invalid-payload-warning "web app only")
                        :warning)))

(defn import-chosen-graph
  [repo]
  (p/let [_ (persist-db/<unsafe-delete repo)]
    (notification/show! (t :graph/updated-switching) :success)
    (state/pub-event! [:graph/switch repo])))

(defn ^:export replace-graph-with-db-file []
  (state/pub-event! [:dialog-select/db-graph-replace]))
