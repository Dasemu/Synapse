(ns frontend.worker.handler.live-notes-test
  (:require [cljs.test :refer [async deftest is testing]]
            [datascript.core :as d]
            [frontend.worker.handler.live-notes :as live-notes]
            [frontend.worker.handler.page :as page-handler]
            [frontend.worker.platform :as platform]
            [frontend.worker.state :as worker-state]
            [logseq.db.test.helper :as db-test]
            [promesa.core :as p]))

(deftest effective-refresh-minutes-clamps-to-a-minimum
  (is (= 60 (live-notes/effective-refresh-minutes nil)))
  (is (= 60 (live-notes/effective-refresh-minutes 60)))
  (is (= 5 (live-notes/effective-refresh-minutes 1)))
  (is (= 30 (live-notes/effective-refresh-minutes 30))))

(deftest due?-is-true-when-never-fetched
  (is (true? (live-notes/due? 1000000 nil 60)))
  (is (true? (live-notes/due? 1000000 "not-a-number" 60))))

(deftest due?-compares-elapsed-time-against-the-refresh-interval
  (let [now 10000000
        one-hour-ms (* 60 60 1000)]
    (testing "not yet due"
      (is (false? (live-notes/due? now (- now (/ one-hour-ms 2)) 60))))
    (testing "exactly due"
      (is (true? (live-notes/due? now (- now one-hour-ms) 60))))
    (testing "overdue"
      (is (true? (live-notes/due? now (- now (* 2 one-hour-ms)) 60))))))

(deftest truncate-content-leaves-short-text-untouched
  (is (= "short" (live-notes/truncate-content "short")))
  (is (nil? (live-notes/truncate-content nil))))

(deftest truncate-content-truncates-long-text-with-a-marker
  (let [long-text (apply str (repeat 9000 "a"))
        result (live-notes/truncate-content long-text)]
    (is (< (count result) (count long-text)))
    (is (re-find #"\(truncated\)$" result))))

(def ^:private test-repo "live-notes-test-repo")

;; :url is a ref-typed property (its value is wrapped in a property-value
;; entity, per db-property-build/build-properties-with-ref-values), so it
;; can't be set with a raw [:db/add eid attr string] tuple, and :build/tags
;; + :build/properties on create-conn-with-blocks' page map only recognizes
;; property/class namespaces curated into logseq-property-namespaces/
;; db-class/logseq-class? -- going through page-handler/create! (the same
;; path caldav.cljs's upsert-page! uses to create tagged pages with
;; properties) exercises the real ref-value-building pipeline instead.
(defn- with-test-conn
  [f]
  (let [conn (db-test/create-conn-with-blocks {:pages-and-blocks []})
        _ (page-handler/create! conn "Weather"
                                {:tags [(d/entity @conn [:db/ident :logseq.class/LiveNote])]
                                 :properties {:logseq.property.livenote/url "https://example.com/weather"
                                              :logseq.property.livenote/refresh-minutes 60}})
        _ (page-handler/create! conn "Plain page" {})
        prev-conns @worker-state/*datascript-conns]
    (swap! worker-state/*datascript-conns assoc test-repo conn)
    (try
      (f conn)
      (finally
        (reset! worker-state/*datascript-conns prev-conns)
        (live-notes/stop-refresh! test-repo)))))

(defn- fake-fetch
  [response-text]
  (fn [_url _opts]
    (js/Promise.resolve
     #js {:ok true
          :text (fn [] (js/Promise.resolve response-text))})))

(defn- failing-fetch
  [_url _opts]
  (js/Promise.resolve #js {:ok false :status 500 :text (fn [] (js/Promise.resolve ""))}))

(def ^:private test-platform
  {:env {:publishing? false :runtime :node}
   :storage {}
   :kv {:get (fn [_] nil) :set! (fn [_ _] nil)}
   :broadcast {:post-message! (fn [& _] nil)}
   :websocket {:connect (fn [_] #js {})}
   :sqlite {}
   :crypto {:save-secret-text! (fn [_key _text] (p/resolved nil))
            :read-secret-text (fn [_key] (p/resolved nil))
            :delete-secret-text! (fn [_key] (p/resolved nil))}
   :timers {:set-interval! (fn [_ _] nil)}})

;; live-notes/<read-auth calls (platform/current), which throws unless the
;; platform adapter is installed for this test process. with-redefs only
;; holds for the SYNCHRONOUS extent of its body -- <tick! resolves through
;; several .then callbacks that run after this fn returns, by which point a
;; with-redefs binding would already be gone (and platform/current back to
;; throwing) -- so install a real platform via set-platform! instead, kept
;; live until the returned promise settles, matching the async-safe
;; save/restore pattern in db_worker_test.cljs's restoring-worker-state.
(defn- with-stub-platform
  "Installs test-platform for the async extent of (f), then restores the
   prior platform and calls done -- in that order, so the next async test
   can't install its own platform before this one's cleanup runs (a race
   that previously clobbered the next test's platform back to nil)."
  [f done]
  (let [platform-prev @@#'platform/*platform]
    (platform/set-platform! test-platform)
    (-> (f)
        (p/catch (fn [e] (is false (str "unexpected error: " e))))
        (p/finally (fn []
                     (reset! @#'platform/*platform platform-prev)
                     (done))))))

(deftest tick-fetches-due-live-notes-and-transacts-content
  (async done
    (with-stub-platform
     (fn []
       (with-test-conn
        (fn [conn]
          (-> (live-notes/<tick! test-repo (fake-fetch "Sunny, 22C"))
              (p/then (fn [_]
                        (let [page (db-test/find-page-by-title @conn "Weather")]
                          (is (= "Sunny, 22C" (:logseq.property.livenote/content page)))
                          (is (number? (:logseq.property.livenote/last-fetched-at page)))
                          (is (nil? (:logseq.property.livenote/last-error page))))))))))
     done)))

(deftest tick-ignores-pages-that-are-not-live-notes
  (async done
    (with-stub-platform
     (fn []
       (with-test-conn
        (fn [conn]
          (-> (live-notes/<tick! test-repo (fake-fetch "irrelevant"))
              (p/then (fn [_]
                        (let [page (db-test/find-page-by-title @conn "Plain page")]
                          (is (nil? (:logseq.property.livenote/content page))))))))))
     done)))

(deftest tick-records-an-error-when-the-fetch-fails
  (async done
    (with-stub-platform
     (fn []
       (with-test-conn
        (fn [conn]
          (-> (live-notes/<tick! test-repo failing-fetch)
              (p/then (fn [_]
                        (let [page (db-test/find-page-by-title @conn "Weather")]
                          (is (nil? (:logseq.property.livenote/content page)))
                          (is (string? (:logseq.property.livenote/last-error page)))
                          (is (number? (:logseq.property.livenote/last-fetched-at page))))))))))
     done)))
