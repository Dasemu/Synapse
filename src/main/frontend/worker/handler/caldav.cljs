(ns frontend.worker.handler.caldav
  "Nextcloud CalDAV (events, tasks) and CardDAV (contacts) sync.

  Deliberately scoped, not a full RFC 4791/6352 client:
  * one calendar collection + one addressbook collection per graph
  * basic auth (Nextcloud app password), no OAuth
  * VEVENT/VTODO/VCARD only, no VALARM/VTIMEZONE/recurrence overrides
  * dates are read/written in UTC only
  * RRULE support covers a single FREQ=DAILY|WEEKLY|MONTHLY|YEARLY(;INTERVAL=n)
    rule, matching only the `plus` (advance-from-scheduled) repeat mode --
    `dotted-plus` (advance-from-completion) and `double-plus` (skip-to-future)
    depend on runtime state RRULE can't express, so those tasks are pushed
    without a recurrence rule and incoming events always come back as `plus`.
  * conflict handling is last-write-wins via ETag: a 412 on push is skipped
    for that cycle and naturally retried after the next pull refreshes the
    stored ETag -- there is no merge UI."
  (:require [clojure.string :as string]
            [datascript.core :as d]
            [frontend.common.thread-api :refer [def-thread-api]]
            [frontend.worker.handler.page :as page-handler]
            [frontend.worker.platform :as platform]
            [frontend.worker.state :as worker-state]
            [lambdaisland.glogi :as log]
            [logseq.db :as ldb]
            [promesa.core :as p]))

;; -----------------------------------------------------------------------
;; Credentials
;; -----------------------------------------------------------------------

(defn- credential-secret-key
  [repo]
  (str "caldav:" repo))

(defn <save-credentials!
  [repo config]
  (platform/save-secret-text! (platform/current) (credential-secret-key repo)
                              (js/JSON.stringify (clj->js config))))

(defn <read-credentials
  [repo]
  (p/let [text (platform/read-secret-text (platform/current) (credential-secret-key repo))]
    (when text
      (js->clj (js/JSON.parse text) :keywordize-keys true))))

(defn <delete-credentials!
  [repo]
  (platform/delete-secret-text! (platform/current) (credential-secret-key repo)))

;; -----------------------------------------------------------------------
;; RRULE <-> :logseq.property.repeat/* translation (pure)
;; -----------------------------------------------------------------------

(def ^:private recur-unit->rrule-freq
  {:logseq.property.repeat/recur-unit.day "DAILY"
   :logseq.property.repeat/recur-unit.week "WEEKLY"
   :logseq.property.repeat/recur-unit.month "MONTHLY"
   :logseq.property.repeat/recur-unit.year "YEARLY"})

(def ^:private rrule-freq->recur-unit
  (into {} (map (fn [[k v]] [v k])) recur-unit->rrule-freq))

(defn repeat->rrule
  "Only the `plus` (advance-from-scheduled) repeat type maps onto RRULE;
   anything else (or an unsupported unit) returns nil, meaning: push this
   task/event without a recurrence rule rather than mistranslate it."
  [{:keys [repeat-type recur-unit recur-frequency]}]
  (when (and (= repeat-type :logseq.property.repeat/repeat-type.plus)
             (contains? recur-unit->rrule-freq recur-unit))
    (let [freq (get recur-unit->rrule-freq recur-unit)
          interval (or recur-frequency 1)]
      (if (> interval 1)
        (str "FREQ=" freq ";INTERVAL=" interval)
        (str "FREQ=" freq)))))

(defn rrule->repeat
  "Only a bare single FREQ (+ optional INTERVAL) is understood. Anything else
   (BYDAY, COUNT, UNTIL, multiple rules...) returns nil so the caller leaves
   the incoming event/task non-recurring instead of silently mistranslating it."
  [rrule-value]
  (when (string? rrule-value)
    (let [parts (into {}
                      (keep (fn [part]
                              (let [[k v] (string/split part #"=" 2)]
                                (when (and k v) [k v]))))
                      (string/split rrule-value #";"))
          freq (get parts "FREQ")]
      (when (and freq
                 (contains? rrule-freq->recur-unit freq)
                 (every? #{"FREQ" "INTERVAL"} (keys parts)))
        {:repeat-type :logseq.property.repeat/repeat-type.plus
         :recur-unit (get rrule-freq->recur-unit freq)
         :recur-frequency (some-> (get parts "INTERVAL") parse-long)}))))

;; -----------------------------------------------------------------------
;; iCalendar / vCard text encoding (pure)
;; -----------------------------------------------------------------------

(defn- ical-escape
  [s]
  (-> (str s)
      (string/replace "\\" "\\\\")
      (string/replace ";" "\\;")
      (string/replace "," "\\,")
      (string/replace "\n" "\\n")))

(defn- ical-unescape
  [s]
  (-> (str s)
      (string/replace "\\n" "\n")
      (string/replace "\\," ",")
      (string/replace "\\;" ";")
      (string/replace "\\\\" "\\")))

(defn- epoch-ms->ical-utc
  [ms]
  (when (number? ms)
    (-> (js/Date. ms)
        .toISOString
        (string/replace #"[-:]" "")
        (string/replace #"\.\d+Z$" "Z"))))

(defn- ical-utc->epoch-ms
  "\"YYYYMMDDTHHMMSSZ\" -> epoch ms. Anything else (local time, dates without
   a time component, non-UTC) is out of scope and returns nil."
  [value]
  (when (and (string? value) (re-matches #"\d{8}T\d{6}Z" value))
    (let [y (subs value 0 4) mo (subs value 4 6) d (subs value 6 8)
          h (subs value 9 11) mi (subs value 11 13) s (subs value 13 15)]
      (.getTime (js/Date. (str y "-" mo "-" d "T" h ":" mi ":" s "Z"))))))

(defn- ical-lines
  [& lines]
  (->> lines flatten (remove nil?) (string/join "\r\n")))

(defn block->vevent
  [{:keys [uid summary description location dtstart dtend rrule]}]
  (ical-lines
   "BEGIN:VCALENDAR" "VERSION:2.0" "PRODID:-//LogDarioSeq//EN"
   "BEGIN:VEVENT"
   (str "UID:" uid)
   (str "SUMMARY:" (ical-escape summary))
   (when dtstart (str "DTSTART:" (epoch-ms->ical-utc dtstart)))
   (when dtend (str "DTEND:" (epoch-ms->ical-utc dtend)))
   (when rrule (str "RRULE:" rrule))
   (when description (str "DESCRIPTION:" (ical-escape description)))
   (when location (str "LOCATION:" (ical-escape location)))
   "END:VEVENT" "END:VCALENDAR"))

(def ^:private status->ical-vtodo-status
  {:logseq.property/status.done "COMPLETED"
   :logseq.property/status.canceled "CANCELLED"
   :logseq.property/status.doing "IN-PROCESS"})

(defn block->vtodo
  [{:keys [uid summary description dtstart due rrule status-ident]}]
  (ical-lines
   "BEGIN:VCALENDAR" "VERSION:2.0" "PRODID:-//LogDarioSeq//EN"
   "BEGIN:VTODO"
   (str "UID:" uid)
   (str "SUMMARY:" (ical-escape summary))
   (when dtstart (str "DTSTART:" (epoch-ms->ical-utc dtstart)))
   (when due (str "DUE:" (epoch-ms->ical-utc due)))
   (when rrule (str "RRULE:" rrule))
   (str "STATUS:" (get status->ical-vtodo-status status-ident "NEEDS-ACTION"))
   (when description (str "DESCRIPTION:" (ical-escape description)))
   "END:VTODO" "END:VCALENDAR"))

(defn contact->vcard
  [{:keys [uid full-name email phone organization]}]
  (ical-lines
   "BEGIN:VCARD" "VERSION:3.0"
   (str "UID:" uid)
   (str "FN:" (ical-escape full-name))
   (when email (str "EMAIL:" email))
   (when phone (str "TEL:" phone))
   (when organization (str "ORG:" (ical-escape organization)))
   "END:VCARD"))

(defn- unfold-ical-lines
  [text]
  (-> (str text)
      (string/replace "\r\n" "\n")
      (string/replace #"\n[ \t]" "")
      (string/split #"\n")))

(def ^:private ical-line-re #"([A-Za-z0-9-]+)(;[^:]*)?:(.*)")

(defn parse-ical-component
  "Parses the first VEVENT or VTODO in an iCalendar text blob into a flat map.
   Deliberately minimal: understands only the property lines this namespace
   itself writes, ignores VALARM/VTIMEZONE/any other component or parameter."
  [text]
  (let [lines (unfold-ical-lines text)
        component-type (some #(second (re-matches #"BEGIN:(VEVENT|VTODO)" %)) lines)]
    (when component-type
      (reduce
       (fn [m line]
         (if-let [[_ k _params v] (re-matches ical-line-re line)]
           (case k
             "UID" (assoc m :uid v)
             "SUMMARY" (assoc m :summary (ical-unescape v))
             "DESCRIPTION" (assoc m :description (ical-unescape v))
             "LOCATION" (assoc m :location (ical-unescape v))
             "DTSTART" (assoc m :dtstart (ical-utc->epoch-ms v))
             "DTEND" (assoc m :dtend (ical-utc->epoch-ms v))
             "DUE" (assoc m :due (ical-utc->epoch-ms v))
             "RRULE" (assoc m :rrule v)
             "STATUS" (assoc m :status v)
             m)
           m))
       {:type component-type}
       lines))))

(defn parse-vcard
  [text]
  (let [lines (unfold-ical-lines text)]
    (when (some #{"BEGIN:VCARD"} lines)
      (reduce
       (fn [m line]
         (if-let [[_ k _params v] (re-matches ical-line-re line)]
           (case k
             "UID" (assoc m :uid v)
             "FN" (assoc m :full-name (ical-unescape v))
             "EMAIL" (assoc m :email v)
             "TEL" (assoc m :phone v)
             "ORG" (assoc m :organization (ical-unescape v))
             m)
           m))
       {}
       lines))))

;; -----------------------------------------------------------------------
;; Minimal multistatus XML extraction (pure, namespace-prefix agnostic)
;; -----------------------------------------------------------------------

(defn- xml-unescape
  [s]
  (-> (str s)
      (string/replace #"&#x([0-9a-fA-F]+);" (fn [[_ hex]] (js/String.fromCharCode (js/parseInt hex 16))))
      (string/replace #"&#(\d+);" (fn [[_ dec]] (js/String.fromCharCode (js/parseInt dec 10))))
      (string/replace "&lt;" "<")
      (string/replace "&gt;" ">")
      (string/replace "&quot;" "\"")
      (string/replace "&apos;" "'")
      (string/replace "&amp;" "&")))

(defn- xml-tag-values
  [tag xml]
  (->> (re-seq (re-pattern (str "<(?:[A-Za-z0-9]+:)?" tag "[^>]*>([\\s\\S]*?)</(?:[A-Za-z0-9]+:)?" tag ">"))
               xml)
       (map (comp xml-unescape second))))

(defn parse-multistatus
  "Extracts {:href :etag :data} per <response> in a CalDAV/CardDAV multistatus
   body. `data-tag` is \"calendar-data\" or \"address-data\"."
  [data-tag multistatus-xml]
  (->> (xml-tag-values "response" multistatus-xml)
       (keep (fn [response-xml]
               (let [href (first (xml-tag-values "href" response-xml))
                     etag (first (xml-tag-values "getetag" response-xml))
                     data (first (xml-tag-values data-tag response-xml))]
                 (when (and href data)
                   {:href href :etag etag :data data}))))))

;; -----------------------------------------------------------------------
;; HTTP (Node runtime only; injectable fetch-fn for testing)
;; -----------------------------------------------------------------------

(defn- join-url
  [base path]
  (str (string/replace base #"/$" "") "/" (string/replace path #"^/" "")))

(defn- basic-auth-header
  [{:keys [username app-password]}]
  (str "Basic " (js/btoa (str username ":" app-password))))

(defn- <dav-request
  [fetch-fn config path {:keys [method headers body]}]
  (-> (fetch-fn (join-url (:base-url config) path)
                (clj->js (merge {:method (or method "GET")
                                 :headers (merge {"Authorization" (basic-auth-header config)}
                                                headers)}
                                (when body {:body body}))))
      (p/then (fn [resp]
                (p/let [text (.text resp)]
                  {:status (.-status resp)
                   :etag (some-> (.-headers resp) (.get "ETag"))
                   :body text})))))

(def ^:private events-query-body
  (ical-lines
   "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
   "<c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">"
   "<d:prop><d:getetag/><c:calendar-data/></d:prop>"
   "<c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"/></c:comp-filter></c:filter>"
   "</c:calendar-query>"))

(def ^:private tasks-query-body
  (ical-lines
   "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
   "<c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">"
   "<d:prop><d:getetag/><c:calendar-data/></d:prop>"
   "<c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VTODO\"/></c:comp-filter></c:filter>"
   "</c:calendar-query>"))

(def ^:private contacts-query-body
  (ical-lines
   "<?xml version=\"1.0\" encoding=\"utf-8\"?>"
   "<c:addressbook-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\">"
   "<d:prop><d:getetag/><c:address-data/></d:prop>"
   "</c:addressbook-query>"))

(defn- <report
  [fetch-fn config path body data-tag]
  (-> (<dav-request fetch-fn config path
                    {:method "REPORT"
                     :headers {"Content-Type" "application/xml; charset=utf-8" "Depth" "1"}
                     :body body})
      (p/then (fn [{:keys [status body]}]
                (if (#{207} status)
                  (parse-multistatus data-tag body)
                  (do (log/warn :caldav/report-failed {:path path :status status})
                      []))))))

(defn- <put!
  [fetch-fn config href text content-type etag]
  (<dav-request fetch-fn config href
               {:method "PUT"
                :headers (cond-> {"Content-Type" content-type}
                           etag (assoc "If-Match" etag))
                :body text}))

;; -----------------------------------------------------------------------
;; Pull: upsert graph pages from remote components
;; -----------------------------------------------------------------------

(defn- find-page-by-caldav-uid
  [db uid]
  (when-let [eid (d/q '[:find ?e . :in $ ?uid
                        :where [?e :logseq.property.caldav/uid ?uid]]
                      db uid)]
    (d/entity db eid)))

(defn- clean-properties
  [m]
  (into {} (remove (comp nil? val)) m))

(defn- caldav-diff-value
  "Compares a datascript entity's current attribute value against an incoming
   value that may be a `[:db/ident kw]` lookup ref (closed-value properties
   like status), unwrapping both sides to their ident before comparing."
  [existing-val new-val]
  (let [existing-ident (:db/ident existing-val)
        new-ident (when (and (vector? new-val) (= :db/ident (first new-val)))
                    (second new-val))]
    (if (or existing-ident new-ident)
      (not= existing-ident new-ident)
      (not= existing-val new-val))))

(defn- upsert-page!
  [conn tag-ident title properties]
  (let [db @conn
        uid (:logseq.property.caldav/uid properties)
        existing (find-page-by-caldav-uid db uid)]
    (if existing
      (let [tx (->> properties
                    (keep (fn [[k v]]
                            (when (caldav-diff-value (get existing k) v)
                              [:db/add (:db/id existing) k v]))))]
        (when (seq tx) (ldb/transact! conn tx))
        (:block/uuid existing))
      (let [[_title page-uuid] (page-handler/create! conn title
                                                      {:tags [(d/entity db [:db/ident tag-ident])]
                                                       :properties (clean-properties properties)})]
        page-uuid))))

(defn- <pull-events!
  [conn fetch-fn config]
  (p/let [items (<report fetch-fn config (:calendar-path config) events-query-body "calendar-data")]
    (doseq [{:keys [href etag data]} items]
      (when-let [{:keys [uid summary dtstart dtend]} (parse-ical-component data)]
        (upsert-page! conn :logseq.class/Event (or summary uid)
                      {:logseq.property.caldav/uid uid
                       :logseq.property.caldav/href href
                       :logseq.property.caldav/etag etag
                       :logseq.property/scheduled dtstart
                       :logseq.property/deadline dtend})))
    (count items)))

(def ^:private vtodo-status->status-ident
  {"COMPLETED" [:db/ident :logseq.property/status.done]
   "CANCELLED" [:db/ident :logseq.property/status.canceled]
   "IN-PROCESS" [:db/ident :logseq.property/status.doing]})

(defn- <pull-tasks!
  [conn fetch-fn config]
  (p/let [items (<report fetch-fn config (:calendar-path config) tasks-query-body "calendar-data")]
    (doseq [{:keys [href etag data]} items]
      (when-let [{:keys [uid summary dtstart due status]} (parse-ical-component data)]
        (upsert-page! conn :logseq.class/Task (or summary uid)
                      (cond-> {:logseq.property.caldav/uid uid
                               :logseq.property.caldav/href href
                               :logseq.property.caldav/etag etag
                               :logseq.property/scheduled dtstart
                               :logseq.property/deadline due}
                        (get vtodo-status->status-ident status)
                        (assoc :logseq.property/status (get vtodo-status->status-ident status))))))
    (count items)))

(defn- <pull-contacts!
  [conn fetch-fn config]
  (p/let [items (<report fetch-fn config (:addressbook-path config) contacts-query-body "address-data")]
    (doseq [{:keys [href etag data]} items]
      (when-let [{:keys [uid full-name email phone organization]} (parse-vcard data)]
        (upsert-page! conn :logseq.class/Contact (or full-name uid)
                      {:logseq.property.caldav/uid uid
                       :logseq.property.caldav/href href
                       :logseq.property.caldav/etag etag
                       :logseq.property.contact/email email
                       :logseq.property.contact/phone phone
                       :logseq.property.contact/organization organization})))
    (count items)))

;; -----------------------------------------------------------------------
;; Push: send local changes for graph pages that carry (or need) a caldav uid
;; -----------------------------------------------------------------------

(defn- pushable-pages
  [db tag-ident]
  (->> (d/datoms db :avet :block/tags tag-ident)
       (map (fn [datom] (d/entity db (:e datom))))
       (remove #(or (:logseq.property/deleted-at %) (ldb/hidden? %)))))

(defn- ensure-uid!
  [conn page]
  (or (:logseq.property.caldav/uid page)
      (let [uid (str (random-uuid))]
        (ldb/transact! conn [[:db/add (:db/id page) :logseq.property.caldav/uid uid]])
        uid)))

(defn- <push-page!
  [conn fetch-fn config collection-path page ->text content-type]
  (p/let [uid (ensure-uid! conn page)
          href (or (:logseq.property.caldav/href page) (join-url collection-path (str uid ".ics")))
          text (->text (assoc page :uid uid))
          {:keys [status etag]} (<put! fetch-fn config href text content-type
                                       (:logseq.property.caldav/etag page))]
    (cond
      (#{200 201 204} status)
      (ldb/transact! conn (remove nil?
                                  [[:db/add (:db/id page) :logseq.property.caldav/href href]
                                   (when etag [:db/add (:db/id page) :logseq.property.caldav/etag etag])]))

      (= 412 status)
      (log/info :caldav/push-conflict {:href href :page-uuid (:block/uuid page)})

      :else
      (log/warn :caldav/push-failed {:href href :status status}))))

(defn- event-page->vevent-input
  [page]
  {:summary (:block/title page)
   :location (:logseq.property.event/location page)
   :dtstart (:logseq.property/scheduled page)
   :dtend (:logseq.property/deadline page)})

(defn- task-page->vtodo-input
  [page]
  {:summary (:block/title page)
   :dtstart (:logseq.property/scheduled page)
   :due (:logseq.property/deadline page)
   :rrule (repeat->rrule {:repeat-type (:db/ident (:logseq.property.repeat/repeat-type page))
                          :recur-unit (:db/ident (:logseq.property.repeat/recur-unit page))
                          :recur-frequency (:logseq.property.repeat/recur-frequency page)})
   :status-ident (:db/ident (:logseq.property/status page))})

(defn- contact-page->vcard-input
  [page]
  {:full-name (:block/title page)
   :email (:logseq.property.contact/email page)
   :phone (:logseq.property.contact/phone page)
   :organization (:logseq.property.contact/organization page)})

(defn- <push-all!
  [conn fetch-fn config]
  (let [db @conn]
    (p/all
     (concat
      (map #(<push-page! conn fetch-fn config (:calendar-path config) %
                         (comp block->vevent event-page->vevent-input) "text/calendar; charset=utf-8")
           (pushable-pages db :logseq.class/Event))
      (map #(<push-page! conn fetch-fn config (:calendar-path config) %
                         (comp block->vtodo task-page->vtodo-input) "text/calendar; charset=utf-8")
           (pushable-pages db :logseq.class/Task))
      (map #(<push-page! conn fetch-fn config (:addressbook-path config) %
                         (comp contact->vcard contact-page->vcard-input) "text/vcard; charset=utf-8")
           (pushable-pages db :logseq.class/Contact))))))

;; -----------------------------------------------------------------------
;; Orchestration
;; -----------------------------------------------------------------------

(def ^:private sync-interval-ms (* 15 60 1000))
(defonce ^:private *sync-intervals (atom {}))
(defonce ^:private *sync-status (atom {}))

(defn- <sync!
  ([repo] (<sync! repo js/fetch))
  ([repo fetch-fn]
   (p/let [config (<read-credentials repo)]
     (if-not config
       (p/resolved {:status :not-connected})
       (let [conn (worker-state/get-datascript-conn repo)]
         (-> (p/let [events (<pull-events! conn fetch-fn config)
                     tasks (<pull-tasks! conn fetch-fn config)
                     contacts (<pull-contacts! conn fetch-fn config)
                     _ (<push-all! conn fetch-fn config)]
               (swap! *sync-status assoc repo {:last-sync-at (js/Date.now()) :last-sync-error nil})
               {:status :ok :events events :tasks tasks :contacts contacts})
             (p/catch (fn [error]
                        (swap! *sync-status assoc repo {:last-sync-at (js/Date.now())
                                                        :last-sync-error (str error)})
                        (log/error :caldav/sync-failed {:repo repo :error (str error)})
                        {:status :error :error (str error)}))))))))

(defn- node-runtime?
  []
  (= :node (platform/env-flag (platform/current) :runtime)))

(defn stop-sync!
  [repo]
  (when-let [interval-id (get @*sync-intervals repo)]
    (js/clearInterval interval-id))
  (swap! *sync-intervals dissoc repo))

(defn maybe-start-sync!
  "Starts the periodic CalDAV/CardDAV sync for `repo` if credentials are
   configured. Node-only: the daemon is the only runtime with outbound
   fetch and keychain access."
  [repo]
  (when (node-runtime?)
    (-> (<read-credentials repo)
        (p/then (fn [config]
                  (when config
                    (stop-sync! repo)
                    (<sync! repo)
                    (swap! *sync-intervals assoc repo
                           (js/setInterval #(<sync! repo) sync-interval-ms)))))
        (p/catch (fn [error]
                   (log/error :caldav/maybe-start-sync-failed {:repo repo :error (str error)}))))))

(def-thread-api :thread-api/caldav-connect!
  [repo config]
  (p/let [_ (<save-credentials! repo config)]
    (maybe-start-sync! repo)
    {:status :connected}))

(def-thread-api :thread-api/caldav-disconnect!
  [repo]
  (stop-sync! repo)
  (swap! *sync-status dissoc repo)
  (p/let [_ (<delete-credentials! repo)]
    {:status :disconnected}))

(def-thread-api :thread-api/caldav-status
  [repo]
  (p/let [config (<read-credentials repo)]
    (merge {:connected? (some? config)
           :base-url (:base-url config)
           :username (:username config)}
          (get @*sync-status repo))))

(def-thread-api :thread-api/caldav-sync-now!
  [repo]
  (<sync! repo))
