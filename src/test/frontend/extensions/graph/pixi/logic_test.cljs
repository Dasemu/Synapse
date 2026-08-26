(ns frontend.extensions.graph.pixi.logic-test
  (:require [cljs.test :refer [deftest is testing]]
            [frontend.extensions.graph.pixi.logic :as logic]))

(deftest temporal-view-mode-lays-out-nodes-on-a-fixed-timeline
  (let [nodes [{:id "a" :kind "page" :block/created-at 300}
               {:id "b" :kind "page" :block/created-at 100}
               {:id "c" :kind "page" :block/created-at 200}]
        links []
        result (logic/layout-nodes nodes links :temporal false)
        by-id (into {} (map (juxt :id identity) result))]
    (testing "nodes are ordered chronologically along the x-axis, oldest first"
      (is (< (:x (by-id "b")) (:x (by-id "c")) (:x (by-id "a")))))
    (testing "every node gets a fixed position and radius, no force simulation"
      (is (every? #(and (number? (:x %)) (number? (:y %)) (number? (:radius %))) result)))
    (testing "re-running layout for the same input is deterministic"
      (is (= result (logic/layout-nodes nodes links :temporal false))))))
