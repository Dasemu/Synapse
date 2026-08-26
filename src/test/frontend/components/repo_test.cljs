(ns frontend.components.repo-test
  (:require [cljs.test :refer [deftest is async]]
            [frontend.components.repo :as repo]
            [frontend.handler.graph :as graph-handler]
            [frontend.state :as state]
            [frontend.util :as util]
            [promesa.core :as p]))

(defn- finish-async-test!
  [done]
  (js/setTimeout done 0))

(deftest open-in-another-tab-action-is-web-only-for-existing-graphs-test
  (let [actionable-f (some-> (resolve 'frontend.components.repo/open-in-another-tab-action?) deref)]
    (is (fn? actionable-f) "All graphs should expose a web open-in-another-tab action predicate")
    (when actionable-f
      (with-redefs [util/web-platform? true]
        (is (true? (actionable-f {:url "logseq_db_demo"
                                  :root "/graphs/demo"
                                  :GraphUUID "graph-uuid"})))
        (is (true? (actionable-f {:url "logseq_db_demo"
                                  :root "/graphs/demo"})))
        (is (false? (actionable-f {:url "logseq_db_remote"}))))
      (with-redefs [util/web-platform? false]
        (is (false? (actionable-f {:url "logseq_db_demo"
                                   :root "/graphs/demo"
                                   :GraphUUID "graph-uuid"})))))))

(deftest open-graph-in-another-tab-publishes-graph-open-window-event-test
  (let [open-f (some-> (resolve 'frontend.components.repo/open-graph-in-another-tab!) deref)
        events (atom [])]
    (is (fn? open-f) "All graphs should expose a web open-in-another-tab action")
    (when open-f
      (with-redefs [state/pub-event! (fn [event]
                                       (swap! events conj event))]
        (open-f {:url "logseq_db_demo"
                 :GraphUUID "graph-uuid"})
        (is (= [[:graph/open-new-window {:repo "logseq_db_demo"
                                          :graph-id "graph-uuid"}]]
               @events))))))

(deftest open-local-graph-in-another-tab-resolves-graph-id-from-registry-test
  (async done
         (let [open-f (some-> (resolve 'frontend.components.repo/open-graph-in-another-tab!) deref)
               events (atom [])]
           (is (fn? open-f) "All graphs should expose a web open-in-another-tab action")
           (if-not open-f
             (finish-async-test! done)
             (-> (p/with-redefs [graph-handler/<get-graph-registry
                                 (fn []
                                   (p/resolved [{:repo "logseq_db_demo"
                                                 :graph-name "demo"
                                                 :graph-id "local-uuid"}]))
                                 state/pub-event! (fn [event]
                                                    (swap! events conj event))]
                   (open-f {:url "logseq_db_demo"
                            :root "/graphs/demo"}))
                 (p/then (fn []
                           (is (= [[:graph/open-new-window {:repo "logseq_db_demo"
                                                             :graph-id "local-uuid"}]]
                                  @events))
                           (finish-async-test! done)))
                 (p/catch (fn [error]
                            (is false (str error))
                            (finish-async-test! done))))))))

(deftest graph-selection-defers-closing-sidebar-at-mobile-breakpoint-test
  (async done
         (let [close-f (some-> (resolve 'frontend.components.repo/close-sidebar-after-repo-popup-action!)
                               deref)
               sidebar-open? (state/get-state :ui/left-sidebar-open?)]
           (is (fn? close-f) "Graph selection should close the mobile sidebar")
           (if-not close-f
             (done)
             (with-redefs [util/sm-breakpoint? (constantly true)]
               (state/set-left-sidebar-open! true)
               (close-f)
               (is (true? (state/get-state :ui/left-sidebar-open?))
                   "Sidebar closing must not unmount the dropdown during its click handler")
               (js/setTimeout
                (fn []
                  (is (false? (state/get-state :ui/left-sidebar-open?)))
                  (state/set-left-sidebar-open! sidebar-open?)
                  (done))
                0))))))

(deftest graph-selection-keeps-desktop-sidebar-state-test
  (let [close-f (some-> (resolve 'frontend.components.repo/close-sidebar-after-repo-popup-action!)
                        deref)
        states (atom [])]
    (is (fn? close-f) "Graph selection should expose the mobile sidebar close action")
    (when close-f
      (with-redefs [util/sm-breakpoint? (constantly false)
                    state/set-left-sidebar-open! #(swap! states conj %)]
        (close-f)
        (is (empty? @states))))))

(deftest repo-popup-action-target-test
  (let [target? (some-> (resolve 'frontend.components.repo/repo-popup-action-target?)
                        deref)
        target (fn [actionable?]
                 #js {:closest (fn [_selector]
                                 (when actionable?
                                   #js {}))})]
    (is (fn? target?) "The graph popup should identify every actionable item")
    (when target?
      (is (true? (target? (target true))))
      (is (false? (target? (target false)))))))

(deftest shift-click-opens-local-graph-in-new-tab-with-registry-graph-id-test
  (async done
         (let [events (atom [])
               links (#'repo/repos-dropdown-links
                      [{:url "logseq_db_demo"
                        :root "/graphs/demo"}]
                      nil)
               on-click (get-in (first links) [:options :on-click])]
           (-> (p/with-redefs [graph-handler/<get-graph-registry
                               (fn []
                                 (p/resolved [{:repo "logseq_db_demo"
                                               :graph-name "demo"
                                               :graph-id "local-uuid"}]))
                               state/pub-event! (fn [event]
                                                  (swap! events conj event))]
                 (on-click #js {:shiftKey true}))
               (p/then (fn []
                         (is (= [[:graph/open-new-window {:repo "logseq_db_demo"
                                                           :graph-id "local-uuid"}]]
                                @events))
                         (finish-async-test! done)))
               (p/catch (fn [error]
                          (is false (str error))
                          (finish-async-test! done)))))))

(deftest electron-shift-click-opens-local-graph-in-new-window-by-repo-test
  (let [events (atom [])
        registry-reads (atom 0)
        links (#'repo/repos-dropdown-links
               [{:url "logseq_db_demo"
                 :root "/graphs/demo"}]
               nil)
        on-click (get-in (first links) [:options :on-click])]
    (with-redefs [util/electron? (constantly true)
                  graph-handler/<get-graph-registry
                  (fn []
                    (swap! registry-reads inc)
                    (p/resolved []))
                  state/pub-event! (fn [event]
                                     (swap! events conj event))]
      (on-click #js {:shiftKey true})
      (is (= [[:graph/open-new-window "logseq_db_demo"]]
             @events))
      (is (zero? @registry-reads)))))
