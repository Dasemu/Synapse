(ns frontend.worker.handler.kernel-test
  (:require [cljs.test :refer [deftest is testing use-fixtures]]
            [frontend.worker.handler.kernel :as kernel]
            [frontend.worker.state :as worker-state]
            [logseq.db.test.helper :as db-test]))

(def ^:private test-repo "kernel-test-repo")

(defn- with-test-conn
  [f]
  (let [conn (db-test/create-conn-with-blocks
              {:pages-and-blocks
               [{:page {:block/title "Kernel page"}
                 :blocks [{:block/title "code block"}]}]})
        prev-conns @worker-state/*datascript-conns]
    (swap! worker-state/*datascript-conns assoc test-repo conn)
    (try
      (f conn)
      (finally
        (reset! worker-state/*datascript-conns prev-conns)
        (kernel/clear-repo-sessions! test-repo)))))

(use-fixtures :each
  {:after #(kernel/clear-repo-sessions! test-repo)})

(deftest eval-block-persists-result-onto-the-block
  (with-test-conn
    (fn [conn]
      (let [block (db-test/find-block-by-content @conn "code block")
            page-uuid (:block/uuid (:block/page block))
            result (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(+ 1 2)")]
        (is (= "3" (:result result)))
        (is (false? (:error? result)))
        (let [updated (db-test/find-block-by-content @conn "code block")]
          (is (= "3" (:logseq.property.code/result updated)))
          (is (number? (:logseq.property.code/executed-at updated))))))))

(deftest eval-block-formats-errors-with-a-prefix
  (with-test-conn
    (fn [conn]
      (let [block (db-test/find-block-by-content @conn "code block")
            page-uuid (:block/uuid (:block/page block))
            result (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(throw (ex-info \"boom\" {}))")]
        (is (true? (:error? result)))
        (is (re-find #"^Error: " (:result result)))))))

(deftest kernel-session-state-persists-across-evaluations-on-the-same-page
  (with-test-conn
    (fn [conn]
      (let [block (db-test/find-block-by-content @conn "code block")
            page-uuid (:block/uuid (:block/page block))]
        (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(def counter 0)")
        (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(def counter (inc counter))")
        (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(def counter (inc counter))")
        (let [result (kernel/eval-block! test-repo page-uuid (:block/uuid block) "counter")]
          (is (= "2" (:result result))))))))

(deftest kernel-sessions-are-isolated-per-page
  (with-test-conn
    (fn [conn]
      (let [block (db-test/find-block-by-content @conn "code block")
            page-a (random-uuid)
            page-b (random-uuid)]
        (kernel/eval-block! test-repo page-a (:block/uuid block) "(def shared 1)")
        (let [result (kernel/eval-block! test-repo page-b (:block/uuid block) "(def shared 2) shared")]
          (is (= "2" (:result result))))
        (let [result (kernel/eval-block! test-repo page-a (:block/uuid block) "shared")]
          (is (= "1" (:result result))))))))

(deftest restart-session-clears-persisted-state
  (with-test-conn
    (fn [conn]
      (let [block (db-test/find-block-by-content @conn "code block")
            page-uuid (:block/uuid (:block/page block))]
        (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(def counter 0)")
        (kernel/restart-session! test-repo page-uuid)
        (let [result (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(def counter 0) counter")]
          (is (= "0" (:result result))))))))

(deftest kernel-namespace-exposes-the-same-helpers-as-the-client-evaluator
  (with-test-conn
    (fn [conn]
      (let [block (db-test/find-block-by-content @conn "code block")
            page-uuid (:block/uuid (:block/page block))]
        (is (= "6" (:result (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(sum [1 2 3])"))))
        (is (= "2" (:result (kernel/eval-block! test-repo page-uuid (:block/uuid block) "(average [1 2 3])"))))))))
