(ns frontend.handler.common.developer-test
  (:require [cljs.test :refer [async deftest is]]
            [clojure.string :as string]
            [frontend.handler.common.developer :as dev-common-handler]
            [frontend.handler.notification :as notification]
            [frontend.state :as state]
            [promesa.core :as p]))

(deftest show-entity-data-uses-worker-pull-test
  (async done
    (let [worker-calls (atom [])
          notifications (atom [])]
      (-> (p/with-redefs [state/get-current-repo (constantly "logseq_db_debug")
                          dev-common-handler/<invoke-db-worker
                          (fn [& args]
                            (swap! worker-calls conj (vec args))
                            (p/resolved {:db/id 42
                                         :block/title "debug block"
                                         :block/refs [{:block/title "ref title"}]}))
                          notification/show!
                          (fn [& args]
                            (swap! notifications conj args))]
            (dev-common-handler/show-entity-data [:block/uuid #uuid "11111111-1111-1111-1111-111111111111"]))
          (p/then
           (fn [_]
             (is (= [[:thread-api/pull
                      "logseq_db_debug"
                      '[*]
                      [:block/uuid #uuid "11111111-1111-1111-1111-111111111111"]]]
                    @worker-calls))
             (is (= 1 (count @notifications)))
             (is (string/includes? (str (ffirst @notifications)) "debug block"))
             (is (string/includes? (str (ffirst @notifications)) "ref title"))))
          (p/catch
           (fn [error]
             (is false (str error))))
          (p/finally done)))))
