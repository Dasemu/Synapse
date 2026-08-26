(ns frontend.db.subs-loader-test
  "Covers the in-flight dedup gap Fase 6 of the server-client migration
  closed: load! deduped calls that landed before a flush, but flush! itself
  dropped the batch (via take-batch!) before the worker request it sent even
  resolved, so a load! arriving while that request was still in the air
  fired a duplicate instead of reusing it.

  state/<invoke-db-worker wraps the actual worker call in a p/let, so even
  its first binding is deferred to a microtask rather than run synchronously
  -- every step below that depends on the mock worker having been called
  needs an explicit (p/delay 0) yield first.

  The mock worker below only tracks :thread-api/get-render-snapshots calls --
  frontend.background-tasks registers its own state/db-worker-ready? watchers
  (online-status/search-idle-status sync) that fire real state/*db-worker
  calls as an unrelated side effect of resetting it here, and this test has
  no business asserting on those."
  (:require [cljs.test :refer [async deftest is use-fixtures]]
            [frontend.db.subs-loader :as loader]
            [frontend.state :as state]
            [promesa.core :as p]))

(def ^:private previous-worker (atom nil))

(use-fixtures :each
  {:before #(reset! previous-worker @state/*db-worker)
   :after #(reset! state/*db-worker @previous-worker)})

(defn- finish-async!
  [done promise]
  (-> promise
      (p/catch (fn [error] (is false (str error))))
      (p/finally done)))

(defn- sync-schedule!
  "Runs the flush on the same tick load! is called, instead of the real
  setTimeout-based schedule-load-batch!."
  [callback]
  (callback))

(deftest load-dedupes-slot-request-across-flush-boundary-test
  (async done
         (let [graph-id "subs-loader-test-graph"
               slot-key [:block (random-uuid)]
               call-count (atom 0)
               pending (atom nil)
               *first (atom nil)
               *second (atom nil)
               *third (atom nil)]
           (reset! state/*db-worker
                   (fn [qkw & args]
                     (if (= qkw :thread-api/get-render-snapshots)
                       (do (swap! call-count inc)
                           (let [deferred (p/deferred)]
                             (reset! pending deferred)
                             deferred))
                       (p/resolved nil))))
           (loader/reset-stats!)
           (finish-async!
            done
            (p/let [_ (do (reset! *first (loader/load! graph-id slot-key sync-schedule!)) nil)
                    _ (p/delay 0)
                    _ (is (= 1 @call-count)
                          "load! sends exactly one worker request for a new slot")
                    _ (do (reset! *second (loader/load! graph-id slot-key sync-schedule!)) nil)
                    _ (p/delay 0)
                    _ (is (= 1 @call-count)
                          "a load! for the same slot while the first request is still
                          in flight -- after flush! already cleared the pending batch
                          -- must reuse the in-flight promise, not fire a duplicate")
                    _ (is (identical? @*first @*second))
                    _ (is (= 1 (:dedup-hits (loader/get-stats))))
                    _ (do (p/resolve! @pending
                                      {:basis-rev 1
                                       :groups {slot-key #{slot-key}}
                                       :slots {slot-key {:value {:block/uuid (second slot-key)}}}})
                          nil)
                    first-result @*first
                    second-result @*second
                    _ (p/delay 0)
                    _ (do (reset! *third (loader/load! graph-id slot-key sync-schedule!)) nil)
                    _ (p/delay 0)]
              (is (= first-result second-result))
              (is (= {:basis-rev 1
                      :slots {slot-key {:value {:block/uuid (second slot-key)}}}}
                     first-result))
              (is (= 2 @call-count)
                  "once the in-flight request settles it's untracked, so a later
                  load! for the same slot fires a fresh request again")
              (is (not (identical? @*third @*first))))))))

(deftest load-dedup-hit-does-not-schedule-a-second-flush-test
  (async done
         (let [graph-id "subs-loader-test-graph-2"
               slot-key [:children (random-uuid)]
               call-count (atom 0)
               schedule-count (atom 0)
               pending (atom nil)]
           (reset! state/*db-worker
                   (fn [qkw & args]
                     (if (= qkw :thread-api/get-render-snapshots)
                       (do (swap! call-count inc)
                           (let [deferred (p/deferred)]
                             (reset! pending deferred)
                             deferred))
                       (p/resolved nil))))
           (loader/reset-stats!)
           (loader/load! graph-id slot-key
                         (fn [callback] (swap! schedule-count inc) (callback)))
           (is (= 1 @schedule-count))
           (loader/load! graph-id slot-key
                         (fn [_callback] (swap! schedule-count inc)))
           (is (= 1 @schedule-count)
               "an in-flight dedup hit must not schedule another flush -- there is
               nothing new to send")
           (finish-async!
            done
            (p/let [_ (p/delay 0)]
              (is (= 1 @call-count))
              (p/resolve! @pending {:basis-rev 1 :groups {slot-key #{slot-key}} :slots {}}))))))
