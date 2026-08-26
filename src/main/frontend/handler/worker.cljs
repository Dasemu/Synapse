(ns frontend.handler.worker
  "Handle messages received from the webworkers"
  (:require [cljs-bean.core :as bean]
            [clojure.string :as string]
            [frontend.common.thread-api :as thread-api]
            [frontend.context.i18n :as i18n]
            [frontend.handler.notification :as notification]
            [frontend.state :as state]
            [lambdaisland.glogi :as log]
            [logseq.db :as ldb]))

(defmulti handle identity)

(defn- normalize-notification-payload
  [[content status clear? uid timeout extra]]
  (let [i18n-data (when (and (map? extra) (contains? extra :i18n-key))
                    extra)
        close-cb (when (fn? extra) extra)]
    {:content content
     :status status
     :clear? clear?
     :uid uid
     :timeout timeout
     :close-cb close-cb
     :i18n-data i18n-data}))

(defmethod handle :notification [_ _worker data]
  (let [{:keys [content status clear? uid timeout close-cb i18n-data]}
        (normalize-notification-payload data)
        translated-content (if-let [i18n-key (:i18n-key i18n-data)]
                             (apply i18n/t i18n-key (or (:i18n-args i18n-data) []))
                             content)]
    (notification/show! translated-content status clear? uid timeout close-cb)))

(defmethod handle :log [_ _worker [name level data]]
  (log/log name level data))

(defmethod handle :add-repo [_ _worker data]
  (state/add-repo! {:url (:repo data)}))

(defmethod handle :asset-file-write-finish [_ _worker {:keys [repo asset-id ts]}]
  (when (and (seq repo) (seq asset-id))
    (state/update-state!
     :assets/asset-file-write-finish
     (fn [m] (assoc-in m [repo asset-id] (or ts (.now js/Date)))))))

(defmethod handle :thread-api/search-index-build-progress [_ _worker args]
  (when-let [f (get @thread-api/*thread-apis :thread-api/search-index-build-progress)]
    (apply f args)))

(defmethod handle :sync-db-changes [_ _worker data]
  (state/pub-event! [:db/sync-changes data]))

(defmethod handle :resync [_ _worker data]
  (state/pub-event! [:db/resync data]))

(defmethod handle :export-current-db [_]
  (state/pub-event! [:db/export-sqlite]))

(defmethod handle :record-worker-client-id [_ _worker data]
  (when-let [client-id (:client-id data)]
    (state/set-db-worker-client-id! client-id)))

(defmethod handle :capture-error [_ _worker data]
  (state/pub-event! [:capture-error data]))

(defmethod handle :backup-file [_ _worker data]
  (state/pub-event! [:graph/backup-file data]))

(defmethod handle :notify-existing-file  [_ _worker data]
  (state/pub-event! [:graph/notify-existing-file data]))

(defmethod handle :default [_ _worker data]
  (prn :debug "Worker data not handled: " data))

(defn- report-worker-error!
  [error-value]
  (let [message (or (:message error-value)
                    (get error-value "message")
                    "Unexpected webworker error")
        error-data (or (:data error-value)
                       (get error-value "data"))
        cause-data (or (get-in error-value [:cause :data])
                       (get-in error-value ["cause" "data"]))]
    (state/pub-event!
     [:capture-error
      {:error (ex-info message (or (when (map? error-data) error-data) {}))
       :payload {:worker-error? true}
       :extra {:worker-error error-value
               :worker-error-data error-data
               :worker-cause-data cause-data}}])))

(defn- suppress-worker-error-log?
  [error-value]
  (= "Non-transact outliner ops contain numeric entity ids"
     (or (:message error-value)
         (get error-value "message"))))

(defn handle-message!
  [^js worker wrapped-worker]
  (assert worker "worker doesn't exists")
  (let [handle-worker-failure!
        (fn [event]
          (when (identical? worker @state/*db-worker-thread)
            (log/error :db-worker/stopped-unexpectedly {:event event})
            (set! (.-onmessage worker) nil)
            (set! (.-onerror worker) nil)
            (set! (.-onmessageerror worker) nil)
            (reset! state/*db-worker-thread nil)
            (reset! state/*db-worker nil)))]
    (set! (.-onerror worker) handle-worker-failure!)
    (set! (.-onmessageerror worker) handle-worker-failure!))
  (set! (.-onmessage worker)
        (fn [event]
          (let [data (.-data event)]
            (if (= data "keepAliveResponse")
              (.postMessage worker "keepAliveRequest")
              (when-not (contains? #{"RAW" "APPLY" "RELEASE"} (.-type data))
                ;; Log thrown exceptions from comlink
                ;; https://github.com/GoogleChromeLabs/comlink/blob/dffe9050f63b1b39f30213adeb1dd4b9ed7d2594/src/comlink.ts#L223-L236
                (if (and (= "HANDLER" (.-type data)) (= "throw" (.-name data)))
                  (if (.-isError (.-value ^js data))
                    (let [error-value (-> data bean/->clj (get-in [:value :value]))]
                      (when-not (suppress-worker-error-log? error-value)
                        (js/console.error "Unexpected webworker error:" error-value)
                        (when-let [stack (:stack error-value)]
                          (js/console.log stack)))
                      (report-worker-error! error-value))
                    (js/console.error "Unexpected webworker error :" data))
                  (if (string? data)
                    (let [[e payload] (ldb/read-transit-str data)]
                      (handle (keyword e) wrapped-worker payload))
                    (when-not (string/starts-with? (.-type data) "MP_")
                      (js/console.error "Worker received invalid data from worker: " data))))))))))
