(ns frontend.worker.db-worker-node-auth
  "Session-based authentication for the db-worker-node HTTP/SSE server.

  A single username/password credential is stored, scrypt-hashed, in
  <root-dir>/db-worker-auth.json. There is no interactive setup yet: the
  credential is bootstrapped from the LOGSEQ_DAEMON_USERNAME /
  LOGSEQ_DAEMON_PASSWORD env vars on first run (see `ensure-credential!`).
  Sessions are bearer tokens held in memory only; a daemon restart revokes
  every session, which is acceptable for a personal single-user server."
  (:require ["crypto" :as crypto]
            ["fs" :as fs]
            ["path" :as node-path]
            [clojure.string :as string]
            [lambdaisland.glogi :as log]))

(defonce ^:private *sessions (atom {}))
(defonce ^:private *failed-attempts (atom {}))

(def ^:private max-failed-attempts 5)
(def ^:private lockout-ms (* 5 60 1000))
(def ^:private scrypt-key-length 64)

(defn- credential-file-path
  [root-dir]
  (node-path/join root-dir "db-worker-auth.json"))

(defn- try-chmod-owner-only!
  [path]
  (try
    (fs/chmodSync path 384) ;; 384 = 0o600, owner read/write only
    (catch :default _ nil)))

(defn- scrypt-hash
  [password salt]
  (.toString (crypto/scryptSync password salt scrypt-key-length) "hex"))

(defn- gen-salt-hex [] (.toString (crypto/randomBytes 16) "hex"))
(defn- gen-token [] (.toString (crypto/randomBytes 32) "base64url"))
(defn- gen-session-id [] (.toString (crypto/randomBytes 8) "hex"))
(defn- now-iso [] (.toISOString (js/Date.)))

(defn- timing-safe-equal?
  "Constant-time string comparison. Guards the length check outside
  crypto.timingSafeEqual, which throws on mismatched buffer lengths instead
  of returning false."
  [a b]
  (let [ba (js/Buffer.from (or a "") "utf8")
        bb (js/Buffer.from (or b "") "utf8")]
    (and (= (.-length ba) (.-length bb))
         (crypto/timingSafeEqual ba bb))))

(defn read-credential
  "Returns {:username :salt :hash :created-at} or nil if none is configured."
  [root-dir]
  (let [path (credential-file-path root-dir)]
    (when (fs/existsSync path)
      (-> (fs/readFileSync path "utf8")
          js/JSON.parse
          (js->clj :keywordize-keys true)))))

(defn set-credential!
  [root-dir username password]
  (let [salt (gen-salt-hex)
        path (credential-file-path root-dir)
        credential {:username username
                    :salt salt
                    :hash (scrypt-hash password salt)
                    :created-at (now-iso)}]
    (fs/writeFileSync path (js/JSON.stringify (clj->js credential)) "utf8")
    (try-chmod-owner-only! path)
    credential))

(defn ensure-credential!
  "Returns the configured credential, bootstrapping one from
  LOGSEQ_DAEMON_USERNAME / LOGSEQ_DAEMON_PASSWORD on first run. Throws if
  none is configured and no bootstrap env vars are set: the daemon must
  refuse to start rather than serve with no way to ever authenticate."
  [root-dir]
  (or (read-credential root-dir)
      (let [username (not-empty (.. js/process -env -LOGSEQ_DAEMON_USERNAME))
            password (not-empty (.. js/process -env -LOGSEQ_DAEMON_PASSWORD))]
        (if (and username password)
          (let [credential (set-credential! root-dir username password)]
            (log/info :db-worker-node-auth-bootstrapped
                      {:username username :path (credential-file-path root-dir)})
            credential)
          (throw (ex-info
                  (str "No db-worker-node credential configured. Set "
                       "LOGSEQ_DAEMON_USERNAME and LOGSEQ_DAEMON_PASSWORD to "
                       "bootstrap one on first run, or restore "
                       (credential-file-path root-dir) ".")
                  {:code :missing-daemon-credential}))))))

(defn- verify-password?
  [{:keys [username salt hash]} candidate-username candidate-password]
  (and hash salt username
       (= username candidate-username)
       (timing-safe-equal? hash (scrypt-hash (or candidate-password "") salt))))

(defn- locked-out?
  [username]
  (when-let [{:keys [locked-until]} (get @*failed-attempts username)]
    (and locked-until (< (js/Date.now) locked-until))))

(defn- record-failed-attempt!
  [username]
  (swap! *failed-attempts update username
         (fn [{:keys [count] :or {count 0}}]
           (let [count' (inc count)]
             (cond-> {:count count'}
               (>= count' max-failed-attempts)
               (assoc :locked-until (+ (js/Date.now) lockout-ms)))))))

(defn- clear-failed-attempts!
  [username]
  (swap! *failed-attempts dissoc username))

(defn create-session!
  [device-label]
  (let [id (gen-session-id)
        now (now-iso)
        session {:id id
                 :token (gen-token)
                 :device-label (or (not-empty device-label) "unknown")
                 :created-at now
                 :last-seen-at now}]
    (swap! *sessions assoc id session)
    session))

(defn login!
  "Verifies username/password against the stored credential and, on
  success, creates a session. Returns {:session {...}} or {:error kw}."
  [root-dir {:keys [username password device-label]}]
  (cond
    (string/blank? username)
    {:error :invalid-credentials}

    (locked-out? username)
    {:error :locked-out}

    :else
    (let [credential (read-credential root-dir)]
      (if (and credential (verify-password? credential username password))
        (do (clear-failed-attempts! username)
            {:session (create-session! device-label)})
        (do (record-failed-attempt! username)
            {:error :invalid-credentials})))))

(defn authorized-token?
  "Given a bare session token (no `Bearer ` prefix), returns true and
  refreshes :last-seen-at when it matches an active session. Shared by the
  Authorization-header path (authorized?) and the WebSocket upgrade path,
  which carries the token via Sec-WebSocket-Protocol instead -- see
  db_worker_node_ws.cljs."
  [token]
  (when (string? token)
    (when-let [session (some #(when (timing-safe-equal? token (:token %)) %)
                             (vals @*sessions))]
      (swap! *sessions assoc-in [(:id session) :last-seen-at] (now-iso))
      true)))

(defn authorized?
  "Given an `Authorization: Bearer <token>` header value (or nil), returns
  true and refreshes :last-seen-at when it matches an active session."
  [authorization-header]
  (when-let [token (some-> authorization-header
                           (as-> h (when (string/starts-with? h "Bearer ") (subs h 7))))]
    (authorized-token? token)))

(defn list-sessions
  []
  (->> (vals @*sessions)
       (map #(dissoc % :token))
       (sort-by :created-at)
       vec))

(defn revoke-session!
  [id]
  (let [existed? (contains? @*sessions id)]
    (swap! *sessions dissoc id)
    existed?))
