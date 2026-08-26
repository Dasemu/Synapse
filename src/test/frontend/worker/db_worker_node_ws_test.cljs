(ns frontend.worker.db-worker-node-ws-test
  "Fast, deterministic tests for the WS resume protocol against a fake
  socket -- no real network, no real daemon. See db_worker_node_test.cljs
  for the real end-to-end WS-over-a-live-daemon coverage."
  (:require [cljs.test :refer [deftest is use-fixtures]]
            [clojure.string :as string]
            [frontend.worker.db-worker-node-ws :as db-ws]))

(use-fixtures :each
  {:before (fn [] (db-ws/close-all!) (db-ws/reset-buffer!))
   :after (fn [] (db-ws/close-all!) (db-ws/reset-buffer!))})

(defn- fake-socket
  []
  (let [listeners (atom {})
        sent (atom [])
        ^js socket #js {:readyState 1
                        :send (fn [text] (swap! sent conj text))
                        :on (fn [event handler]
                              (swap! listeners update (keyword event) (fnil conj []) handler))
                        :ping (fn [] nil)}]
    (set! (.-terminate socket) (fn [] (set! (.-readyState socket) 3)))
    {:sent sent
     :listeners listeners
     :socket socket}))

(defn- fire!
  [{:keys [listeners]} event & args]
  (doseq [handler (get @listeners (keyword event))]
    (apply handler args)))

(defn- sent-json
  [{:keys [sent]}]
  (mapv #(js->clj (js/JSON.parse %) :keywordize-keys true) @sent))

(deftest token-from-upgrade-req-extracts-bare-token
  (is (= "abc123"
         (db-ws/token-from-upgrade-req
          #js {:headers #js {"sec-websocket-protocol" "bearer.abc123"}})))
  (is (nil? (db-ws/token-from-upgrade-req
             #js {:headers #js {"sec-websocket-protocol" "not-bearer-prefixed"}})))
  (is (nil? (db-ws/token-from-upgrade-req #js {:headers #js {}}))))

(deftest reject-upgrade-writes-status-and-destroys-socket
  (let [written (atom nil)
        destroyed (atom false)
        socket #js {:write (fn [text] (reset! written text))
                    :destroy (fn [] (reset! destroyed true))}]
    (db-ws/reject-upgrade! socket 401 "unauthorized")
    (is (string/starts-with? @written "HTTP/1.1 401 unauthorized"))
    (is @destroyed)))

(deftest subscribe-caught-up-replies-subscribed-without-replay
  (let [{:keys [socket] :as fake} (fake-socket)]
    (db-ws/handle-connection! socket "repo-a")
    (db-ws/record-delta! 10 "{\"type\":\"sync-db-changes\",\"payload\":\"rev-10\"}")
    (fire! fake "message"
           (js/JSON.stringify (clj->js {:type "subscribe" :graphId "repo-a" :sinceRev 10})))
    (is (= [{:type "subscribed" :rev 10}] (sent-json fake)))))

(deftest subscribe-behind-head-replays-buffered-deltas-in-order
  (let [{:keys [socket] :as fake} (fake-socket)]
    (db-ws/record-delta! 10 "{\"type\":\"sync-db-changes\",\"payload\":\"rev-10\"}")
    (db-ws/record-delta! 11 "{\"type\":\"sync-db-changes\",\"payload\":\"rev-11\"}")
    (db-ws/record-delta! 12 "{\"type\":\"sync-db-changes\",\"payload\":\"rev-12\"}")
    (db-ws/handle-connection! socket "repo-a")
    (fire! fake "message"
           (js/JSON.stringify (clj->js {:type "subscribe" :graphId "repo-a" :sinceRev 10})))
    (is (= ["{\"type\":\"sync-db-changes\",\"payload\":\"rev-11\"}"
            "{\"type\":\"sync-db-changes\",\"payload\":\"rev-12\"}"]
           (butlast @(:sent fake))))
    (is (= {:type "subscribed" :rev 12} (last (sent-json fake))))))

(deftest subscribe-with-gap-past-buffer-window-triggers-resync
  (let [{:keys [socket] :as fake} (fake-socket)]
    (db-ws/record-delta! 500 "{\"type\":\"sync-db-changes\",\"payload\":\"rev-500\"}")
    (db-ws/handle-connection! socket "repo-a")
    (fire! fake "message"
           (js/JSON.stringify (clj->js {:type "subscribe" :graphId "repo-a" :sinceRev 100})))
    (is (= [{:type "resync" :rev 500}] (sent-json fake)))))

(deftest subscribe-mismatched-graph-id-errors-without-replay-or-resync
  (let [{:keys [socket] :as fake} (fake-socket)]
    (db-ws/record-delta! 5 "{\"type\":\"sync-db-changes\",\"payload\":\"rev-5\"}")
    (db-ws/handle-connection! socket "repo-a")
    (fire! fake "message"
           (js/JSON.stringify (clj->js {:type "subscribe" :graphId "repo-b" :sinceRev 0})))
    (is (= [{:type "error" :message "graph-id does not match bound repo"}]
           (sent-json fake)))))

(deftest broadcast-reaches-only-currently-connected-sockets
  (let [fake-1 (fake-socket)
        fake-2 (fake-socket)]
    (db-ws/handle-connection! (:socket fake-1) "repo-a")
    (db-ws/broadcast! "{\"type\":\"notification\"}")
    (db-ws/handle-connection! (:socket fake-2) "repo-a")
    (db-ws/broadcast! "{\"type\":\"notification-2\"}")
    (is (= ["{\"type\":\"notification\"}" "{\"type\":\"notification-2\"}"] @(:sent fake-1)))
    (is (= ["{\"type\":\"notification-2\"}"] @(:sent fake-2)))))

(deftest close-removes-socket-from-broadcast-set
  (let [fake (fake-socket)]
    (db-ws/handle-connection! (:socket fake) "repo-a")
    (fire! fake "close")
    (db-ws/broadcast! "{\"type\":\"notification\"}")
    (is (= [] @(:sent fake)))))

(deftest ping-message-replies-pong
  (let [fake (fake-socket)]
    (db-ws/handle-connection! (:socket fake) "repo-a")
    (fire! fake "message" (js/JSON.stringify (clj->js {:type "ping"})))
    (is (= [{:type "pong"}] (sent-json fake)))))

(deftest unknown-message-type-replies-error
  (let [fake (fake-socket)]
    (db-ws/handle-connection! (:socket fake) "repo-a")
    (fire! fake "message" (js/JSON.stringify (clj->js {:type "bogus"})))
    (is (= [{:type "error" :message "unknown message type"}] (sent-json fake)))))

(deftest invalid-json-message-replies-error-without-throwing
  (let [fake (fake-socket)]
    (db-ws/handle-connection! (:socket fake) "repo-a")
    (fire! fake "message" "not json")
    (is (= [{:type "error" :message "invalid message"}] (sent-json fake)))))
