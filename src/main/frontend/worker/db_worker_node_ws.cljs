(ns frontend.worker.db-worker-node-ws
  "WebSocket event channel for db-worker-node -- kept alongside the SSE
  channel (/v1/events) during the Fase 4 migration so either transport can
  be rolled back independently; see docs/agent-guide.

  Wire format for domain events mirrors SSE exactly: {\"type\" ... \"payload\"
  <transit-string>} -- db_worker_node.cljs builds that message once (for
  both transports) and hands it to broadcast!/record-delta! here.

  Client -> server control message: {\"type\" \"subscribe\" \"graphId\" repo
  \"sinceRev\" rev-or-null}. Server -> client control messages:
  {\"type\" \"subscribed\" \"rev\" head} (caught up, or replay was just sent),
  {\"type\" \"resync\" \"rev\" head} (client must discard cached state and
  reload everything -- see frontend.handler.events :db/resync),
  {\"type\" \"error\" \"message\" ...}.

  The bearer token can't travel in a header on the WS handshake the way
  fetch requests use Authorization -- carried via Sec-WebSocket-Protocol
  instead (see token-from-upgrade-req), which doesn't leak into proxy/access
  logs the way the SSE interim query-param token does."
  (:require [clojure.string :as string]
            [lambdaisland.glogi :as log]))

(def ^:private buffer-capacity 500)
(def ^:private heartbeat-interval-ms 30000)

(defonce ^:private *clients (atom #{}))
(defonce ^:private *delta-buffer (atom (sorted-map)))
(defonce ^:private *head-rev (atom 0))

(defn- ws-open?
  [^js socket]
  (= 1 (.-readyState socket)))

(defn- ws-send!
  [^js socket text]
  (when (ws-open? socket)
    (try
      (.send socket text)
      (catch :default e
        (log/error :db-worker-node-ws-send-failed e)))))

(defn broadcast!
  "Sends a pre-built wire message (see db_worker_node.cljs handle-event!) to
  every currently connected WS client, live-tailing style. Independent of
  record-delta! -- non-graph events (notifications, logs, ...) broadcast
  without ever entering the resume buffer."
  [message]
  (doseq [^js socket @*clients]
    (ws-send! socket message)))

(defn record-delta!
  "Keeps the last `buffer-capacity` :sync-db-changes wire messages, indexed
  by renderer revision, so a client reconnecting within that window can
  resume via replay instead of a full reload."
  [rev message]
  (when (and (integer? rev) (not (neg? rev)))
    (swap! *delta-buffer
           (fn [buf]
             (let [buf' (assoc buf rev message)]
               (if (> (count buf') buffer-capacity)
                 (dissoc buf' (first (keys buf')))
                 buf'))))
    (swap! *head-rev max rev)))

(defn- oldest-buffered-rev
  []
  (some-> @*delta-buffer first key))

(defn- can-replay-from?
  [since-rev]
  (let [oldest (oldest-buffered-rev)]
    (or (nil? oldest) (<= oldest (inc since-rev)))))

(defn- replay-messages
  [since-rev]
  (->> @*delta-buffer
       (drop-while (fn [[rev _message]] (<= rev since-rev)))
       (map second)))

(defn- send-control!
  [^js socket payload]
  (ws-send! socket (js/JSON.stringify (clj->js payload))))

(defn- handle-subscribe!
  [^js socket bound-repo {:strs [graphId sinceRev]}]
  (if (and (string? graphId) (not= graphId bound-repo))
    (send-control! socket {:type "error" :message "graph-id does not match bound repo"})
    (let [since-rev (if (integer? sinceRev) sinceRev -1)
          head @*head-rev]
      (if (can-replay-from? since-rev)
        (do
          (doseq [message (replay-messages since-rev)]
            (ws-send! socket message))
          (send-control! socket {:type "subscribed" :rev head}))
        (send-control! socket {:type "resync" :rev head})))))

(defn- handle-message!
  [^js socket bound-repo text]
  (try
    (let [data (js->clj (js/JSON.parse text))]
      (case (get data "type")
        "subscribe" (handle-subscribe! socket bound-repo data)
        "ping" (send-control! socket {:type "pong"})
        (send-control! socket {:type "error" :message "unknown message type"})))
    (catch :default e
      (log/error :db-worker-node-ws-message-failed e)
      (send-control! socket {:type "error" :message "invalid message"}))))

(defn- start-heartbeat!
  "Terminates sockets that stop answering pings -- mobile networks in
  particular drop connections silently, and a dead entry in *clients would
  otherwise sit there forever, still counted by broadcast!."
  [^js socket]
  (let [alive? (atom true)
        interval (js/setInterval
                  (fn []
                    (if @alive?
                      (do (reset! alive? false)
                          (try (.ping socket) (catch :default _ nil)))
                      (try (.terminate socket) (catch :default _ nil))))
                  heartbeat-interval-ms)]
    (.on socket "pong" (fn [] (reset! alive? true)))
    interval))

(defn handle-connection!
  [^js socket bound-repo]
  (swap! *clients conj socket)
  (let [heartbeat (start-heartbeat! socket)
        cleanup! (fn []
                   (js/clearInterval heartbeat)
                   (swap! *clients disj socket))]
    (.on socket "message"
         (fn [data]
           (handle-message! socket bound-repo (if (string? data) data (.toString data)))))
    (.on socket "close" cleanup!)
    (.on socket "error" (fn [error]
                          (log/error :db-worker-node-ws-error error)
                          (cleanup!)))))

(defn token-from-upgrade-req
  "Extracts the bare session token from Sec-WebSocket-Protocol. The client
  sends a single subprotocol string \"bearer.<token>\" (see
  frontend.persist-db.remote/open-websocket)."
  [^js req]
  (when-let [header (some-> req .-headers (aget "sec-websocket-protocol"))]
    (let [first-protocol (-> header (string/split #",") first string/trim)]
      (when (string/starts-with? first-protocol "bearer.")
        (subs first-protocol (count "bearer."))))))

(defn reject-upgrade!
  [^js socket status reason]
  (try
    (.write socket
            (str "HTTP/1.1 " status " " reason "\r\n"
                 "Connection: close\r\n"
                 "Content-Type: application/json\r\n\r\n"
                 (js/JSON.stringify (clj->js {:ok false :error {:message reason}}))))
    (catch :default _ nil))
  (try (.destroy socket) (catch :default _ nil)))

(defn close-all!
  []
  (doseq [^js socket @*clients]
    (try (.terminate socket) (catch :default _ nil)))
  (reset! *clients #{}))

(defn reset-buffer!
  []
  (reset! *delta-buffer (sorted-map))
  (reset! *head-rev 0))
