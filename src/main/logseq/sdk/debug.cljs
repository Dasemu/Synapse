(ns ^:no-doc logseq.sdk.debug
  (:require [frontend.state :as state]
            [cljs-bean.core :as bean]))

(defn ^:export log_app_state
  [path]
  (-> (if (string? path)
        (get (state/get-state) (keyword path))
        (state/get-state))
      (bean/->js)))
