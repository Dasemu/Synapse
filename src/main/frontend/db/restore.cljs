(ns frontend.db.restore
  "Fns for DB restore(from text or sqlite)"
  (:require [cljs-time.core :as t]
            [frontend.db.subs :as db-subs]
            [frontend.persist-db :as persist-db]
            [frontend.state :as state]
            [lambdaisland.glogi :as log]
            [promesa.core :as p]))

(defn restore-graph!
  "Restore db from SQLite"
  [repo & {:as opts}]
  (state/set-state! :graph/loading? true)
  (->
   (p/let [start-time (t/now)
           {:keys [schema]} (persist-db/<open-and-fetch-schema repo opts)
           _ (when (nil? schema)
               (throw (ex-info "No valid schema found when reloading db" {:repo repo})))
           _ (state/set-current-repo! repo)
           _ (db-subs/reset-graph! repo)
           end-time (t/now)]
     (log/info ::restore-graph!
               (str "opens worker graph in "
                    (t/in-millis (t/interval start-time end-time)) " ms"))
     (state/pub-event! [:graph/restored repo])
     (state/pub-event! [:ui/re-render-root]))
   (p/finally #(state/set-state! :graph/loading? false))))
