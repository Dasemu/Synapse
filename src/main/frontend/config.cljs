(ns frontend.config
  "App config and fns built on top of configuration"
  (:require [clojure.set :as set]
            [clojure.string :as string]
            [frontend.mobile.secure-storage :as secure-storage]
            [frontend.mobile.util :as mobile-util]
            [frontend.state :as state]
            [frontend.util :as util]
            [goog.crypt.Md5]
            [lambdaisland.glogi :as log]
            [logseq.common.cognito-config :as cognito-config]
            [logseq.common.config :as common-config]
            [logseq.common.graph-dir :as common-graph-dir]
            [logseq.common.path :as path]
            [logseq.db.sqlite.util :as sqlite-util]
            [promesa.core :as p]
            [shadow.resource :as rc]))

(goog-define DEV-RELEASE false)
(defonce dev-release? DEV-RELEASE)
(defonce dev? ^boolean (or dev-release? goog.DEBUG))

(defonce publishing? common-config/PUBLISHING)

;; this is a feature flag to enable the account tab
;; when it launches (when pro plan launches) it should be removed
(def ENABLE-SETTINGS-ACCOUNT-TAB false)

(def COGNITO-CLIENT-ID cognito-config/COGNITO-CLIENT-ID)
(def OAUTH-DOMAIN cognito-config/OAUTH-DOMAIN)

(def API-DOMAIN "api.logseq.com")
(def COGNITO-IDP "https://cognito-idp.us-east-1.amazonaws.com/")
(def REGION "us-east-1")
(def USER-POOL-ID "us-east-1_dtagLnju8")
(def IDENTITY-POOL-ID "us-east-1:d6d3b034-1631-402b-b838-b44513e93ee0")
(def default-publish-api-base "https://logseq.io")

;; Enable for local development
;; (def default-publish-api-base "http://localhost:8787")

(defn valid-sync-server-url?
  "Return true when `url` looks like a valid HTTP(S) base URL."
  [url]
  (and (string? url)
       (re-find #"^https?://" url)))

;; Web/mobile clients connect to a self-hosted db-worker-node daemon by
;; base URL + bearer token, instead of Electron's IPC-managed local runtime
;; (see frontend.persist-db/remote-runtime?). Stored client-side only; the
;; daemon holds the authoritative credential (see
;; frontend.worker.db-worker-node-auth).
;; On native mobile, WebView localStorage isn't durable the way it is on
;; desktop -- iOS in particular can evict it under storage pressure, and it
;; doesn't reliably survive an app reinstall/restore. localStorage stays the
;; synchronous read path everywhere, including mobile (every getter below is
;; called synchronously all over persist_db.cljs/handler.assets.cljs, and
;; retrofitting those call sites to await an async native store is out of
;; scope here); frontend.mobile.secure-storage (OS keychain/keystore) mirrors
;; writes as a best-effort durability backstop and <restore-server-config!
;; below copies a surviving value back into localStorage on next launch if
;; it came back empty (Fase 7). Reuses the existing secure-storage wrapper
;; (its key prefix says "e2ee" internally -- that's the plugin's storage
;; namespace, not a claim about what's stored under it) rather than adding a
;; second native-storage integration for one more kind of secret.
(def ^:private secure-server-url-key "db-worker-server-url")
(def ^:private secure-server-token-key "db-worker-server-token")

(defn- native-mobile?
  "mobile-util/native-platform? touches the Capacitor bridge, which doesn't
   exist under Node (tests, the daemon) or a plain browser tab -- treat any
   throw there the same as \"not native\" rather than letting it take down
   every config.cljs caller, most of which run in those environments."
  []
  (try
    (boolean (mobile-util/native-platform?))
    (catch :default _ false)))

(defn- <mirror-server-config-to-secure-storage!
  [key value]
  (if (native-mobile?)
    (-> (if (string/blank? value)
          (secure-storage/<remove-item! key)
          (secure-storage/<set-item! key value))
        (p/catch (fn [e]
                   (log/warn :config/secure-storage-mirror-failed {:key key :error e}))))
    (p/resolved nil)))

(defn <restore-server-config-from-secure-storage!
  "Best-effort recovery for native mobile: if localStorage lost the
   configured server URL/token but the OS keychain/keystore still has it,
   copy it back into localStorage. Call once at mobile app startup, before
   the client decides whether it's remote-configured; a no-op everywhere
   else."
  []
  (if (native-mobile?)
    (-> (p/let [need-url? (string/blank? (.getItem js/localStorage "db-worker-server-url"))
                need-token? (string/blank? (.getItem js/localStorage "db-worker-server-token"))
                stored-url (when need-url? (secure-storage/<get-item secure-server-url-key))
                stored-token (when need-token? (secure-storage/<get-item secure-server-token-key))]
          (when (seq stored-url)
            (.setItem js/localStorage "db-worker-server-url" stored-url))
          (when (seq stored-token)
            (.setItem js/localStorage "db-worker-server-token" stored-token))
          nil)
        (p/catch (fn [e]
                   (log/warn :config/secure-storage-restore-failed {:error e}))))
    (p/resolved nil)))

(defn get-server-base-url
  "Read the configured db-worker-node server base URL from localStorage.
   Returns nil when not set or empty."
  []
  (when-not util/node-test?
    (let [v (.getItem js/localStorage "db-worker-server-url")]
      (when (and (string? v) (not (string/blank? v)))
        v))))

(defn set-server-base-url!
  "Persist the db-worker-node server base URL to localStorage. Pass nil or
   empty string to clear."
  [url]
  (when-not util/node-test?
    (let [trimmed (some-> url string/trim)]
      (if (string/blank? trimmed)
        (.removeItem js/localStorage "db-worker-server-url")
        (.setItem js/localStorage "db-worker-server-url" trimmed))
      (<mirror-server-config-to-secure-storage! secure-server-url-key trimmed))))

(defn get-server-token
  "Read the current db-worker-node session bearer token from localStorage.
   Returns nil when not set or empty."
  []
  (when-not util/node-test?
    (let [v (.getItem js/localStorage "db-worker-server-token")]
      (when (and (string? v) (not (string/blank? v)))
        v))))

(defn set-server-token!
  "Persist the db-worker-node session bearer token to localStorage. Pass
   nil or empty string to clear (e.g. on logout or a 401)."
  [token]
  (when-not util/node-test?
    (let [trimmed (some-> token string/trim)]
      (if (string/blank? trimmed)
        (.removeItem js/localStorage "db-worker-server-token")
        (.setItem js/localStorage "db-worker-server-token" trimmed))
      (<mirror-server-config-to-secure-storage! secure-server-token-key trimmed))))

(defn clear-server-config!
  "Clear both the configured server URL and session token, e.g. on logout."
  []
  (set-server-base-url! nil)
  (set-server-token! nil))

(defn get-custom-publish-server-url
  "Read the user-configured custom publish server URL from localStorage.
   Returns nil when not set or empty."
  []
  (when-not util/node-test?
    (let [v (.getItem js/localStorage "publish-server-url")]
      (when (and (string? v) (not (string/blank? v)))
        v))))

(defn set-custom-publish-server-url!
  "Persist the custom publish server URL to localStorage. Pass nil or empty string to clear."
  [url]
  (when-not util/node-test?
    (if (or (nil? url) (string/blank? url))
      (.removeItem js/localStorage "publish-server-url")
      (.setItem js/localStorage "publish-server-url" (string/trim url)))))

(defn valid-publish-server-url?
  "Return true when `url` looks like a valid HTTP(S) base URL."
  [url]
  (and (string? url)
       (re-find #"^https?://" url)))

(defn custom-url->publish-api-base
  "Normalize a custom publish base URL by stripping trailing slashes. Pure function."
  [custom-url]
  (string/replace custom-url #"/+$" ""))

(defn publish-api-base
  "Return the base URL for the single-page publish service. Uses the user-configured
   URL from localStorage when set, otherwise the default url above. Read on each call so URL changes take effect without a restart."
  []
  (if-let [custom (get-custom-publish-server-url)]
    (custom-url->publish-api-base custom)
    default-publish-api-base))

;; Feature flags
;; =============

(goog-define ENABLE-PLUGINS true)
(defonce feature-plugin-system-on? ENABLE-PLUGINS)

;; Desktop only as other platforms requires better understanding of their
;; multi-graph workflows and optimal place for a "global" dir
(def global-config-enabled? util/electron?)

;; User level configuration for whether plugins are enabled
(defonce lsp-enabled?
  (and util/plugin-platform?
       (not (false? feature-plugin-system-on?))
       (state/lsp-enabled?-or-theme)))

(defn plugin-config-enabled?
  []
  (and lsp-enabled? (global-config-enabled?)))

;; :TODO: How to do this?
;; (defonce desktop? ^boolean goog.DESKTOP)

;; ============

(def app-name common-config/app-name)

;; FIXME:
(def app-website
  (if dev?
    "http://localhost:3001"
    (util/format "https://%s.com" app-name)))

(def markup-formats
  #{:org :md :markdown :asciidoc :adoc :rst})

(def doc-formats
  #{:doc :docx :xls :xlsx :ppt :pptx :one :pdf :epub})

(def image-formats
  #{:png :jpg :jpeg :bmp :gif :webp :svg :heic :avif :cr2})

(def audio-formats
  #{:mp3 :ogg :mpeg :wav :m4a :flac :wma :aac})

(def video-formats
  #{:mp4 :webm :mov :flv :avi :mkv})

(def media-formats (set/union (common-config/img-formats) audio-formats video-formats))

(def mobile?
  "Triggering condition: Mobile phones
   *** Warning!!! ***
   For UX logic only! Don't use for FS logic
   iPad / Android Pad doesn't trigger!

   Same as config/mobile?"
  (when-not util/node-test?
    (util/safe-re-find #"Mobi" js/navigator.userAgent)))

(defn get-hr
  [format]
  (let [format (or format (keyword (state/get-preferred-format)))]
    (case format
      :markdown
      "---"
      "")))

(defn get-bold
  [format]
  (let [format (or format (keyword (state/get-preferred-format)))]
    (case format
      :markdown
      "**"
      "")))

(defn get-italic
  [format]
  (let [format (or format (keyword (state/get-preferred-format)))]
    (case format
      :markdown
      "*"
      "")))
(defn get-underline
  [format]
  (let [format (or format (keyword (state/get-preferred-format)))]
    (case format
      :markdown ;; no underline for markdown
      ""
      "")))
(defn get-strike-through
  [format]
  (let [format (or format (keyword (state/get-preferred-format)))]
    (case format
      :markdown
      "~~"
      "")))

(defn get-highlight
  [format]
  (case format
    :markdown
    "=="
    ""))

(defn get-code
  [format]
  (let [format (or format (keyword (state/get-preferred-format)))]
    (case format
      :markdown
      "`"
      "")))

(defn get-empty-link-and-forward-pos
  [format]
  (case format
    :markdown
    ["[]()" 1]
    ["" 0]))

(defn link-format
  [label link]
  (if (not-empty label)
    (util/format "[%s](%s)" label link)
    link))

(defn with-default-link
  [format link]
  (case format
    :markdown
    [(util/format "[](%s)" link)
     1]
    ["" 0]))

(defn with-label-link
  [format label link]
  (case format
    :markdown
    [(util/format "[%s](%s)" label link)
     (+ 4 (count link) (count label))]
    ["" 0]))

(defn with-default-label
  [format label]
  (case format
    :markdown
    [(util/format "[%s]()" label)
     (+ 3 (count label))]
    ["" 0]))

(defonce demo-repo "Demo")

(defn demo-graph?
  "Demo graph or nil graph?"
  ([]
   (demo-graph? (state/get-current-repo)))
  ([repo-url]
   (or (nil? repo-url) (= repo-url demo-repo)
       (string/ends-with? repo-url demo-repo))))

(def config-file "config.edn")
(def custom-css-file "custom.css")
(def export-css-file "export.css")
(def custom-js-file "custom.js")
(def config-default-content (rc/inline "templates/config.edn"))

;; NOTE: repo-url is the unique identifier of a repo.
;; - `logseq_db_GraphName` => db based graph, sqlite as backend
;; - Use `""` while writing global files

(defonce db-version-prefix common-config/db-version-prefix)

(defn db-graph-name
  [repo-with-prefix]
  (common-config/strip-leading-db-version-prefix repo-with-prefix))

(defn db-based-graph?
  ([]
   (db-based-graph? (state/get-current-repo)))
  ([s]
   (boolean
    (and (string? s)
         (sqlite-util/db-based-graph? s)))))

(defn get-local-asset-absolute-path
  [s]
  (str "/" (string/replace s #"^[./]*" "")))

(defn get-local-dir
  [repo]
  (path/path-join (get-in (state/get-state) [:system/info :home-dir])
                  "logseq"
                  "graphs"
                  (common-graph-dir/repo->encoded-graph-dir-name repo)))

(defn get-electron-backup-dir
  [repo]
  (path/path-join (get-local-dir repo) "backup"))

(defn get-repo-dir
  [repo-url]
  (when repo-url
    (if (util/electron?)
      (get-local-dir repo-url)
      (str "memory:///"
           (db-graph-name repo-url)))))

(defn get-repo-config-path
  []
  (path/path-join app-name config-file))

(defn get-custom-css-path
  ([]
   (get-custom-css-path (state/get-current-repo)))
  ([repo]
   (if (db-based-graph? repo)
     (path/path-join app-name custom-css-file)
     (when-let [repo-dir (get-repo-dir repo)]
       (path/path-join repo-dir app-name custom-css-file)))))

(defn get-export-css-path
  ([]
   (get-export-css-path (state/get-current-repo)))
  ([repo]
   (when-let [repo-dir (get-repo-dir repo)]
     (path/path-join repo-dir app-name  export-css-file))))

(defn get-repo-assets-root
  [repo]
  (when-let [repo-dir (get-repo-dir repo)]
    (path/path-join repo-dir "assets")))

(defn get-current-repo-assets-root
  []
  (get-repo-assets-root (state/get-current-repo)))

(defn get-custom-js-path
  ([]
   (get-custom-js-path (state/get-current-repo)))
  ([repo]
   (if (db-based-graph? repo)
     (path/path-join app-name custom-js-file)
     (when-let [repo-dir (get-repo-dir repo)]
       (path/path-join repo-dir app-name custom-js-file)))))
