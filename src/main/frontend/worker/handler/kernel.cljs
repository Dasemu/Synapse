(ns frontend.worker.handler.kernel
  "A stateful SCI kernel for executable code blocks: one interpreter context
   per page, reused across block evaluations on that page so `def`s and
   other side effects persist between runs -- unlike the plain, stateless
   per-call eval in frontend.extensions.sci that the block editor already
   does for inline `:results` display.

   Isolation boundary, chosen deliberately instead of guessed at: this does
   NOT expand what code can do beyond what SCI already exposed to the
   client-side single-shot evaluator -- no shell/process execution, no
   filesystem access, no arbitrary JS interop beyond the small curated
   `user` namespace below (a strict subset of frontend.extensions.sci's,
   with the browser-only `call-api` binding dropped). Running the kernel in
   the daemon rather than the browser tab is what makes results consistent
   across clients and lets a session survive a page staying open elsewhere;
   it is not a new sandbox. A real subprocess-based sandbox (per the plan's
   own \"definir el limite antes de implementar\") is future work, not
   something to fake here."
  (:require [datascript.core :as d]
            [frontend.common.thread-api :refer [def-thread-api]]
            [frontend.worker.state :as worker-state]
            [lambdaisland.glogi :as log]
            [logseq.db :as ldb]
            [sci.core :as sci]))

(def ^:private sum (partial apply +))

(defn- average
  [coll]
  (/ (reduce + coll) (count coll)))

(def ^:private default-user-namespace
  {'sum sum
   'average average
   'parseFloat js/parseFloat
   'isNaN js/isNaN
   'log js/console.log
   'pprint pr-str})

(defonce ^:private *sessions (atom {}))

(defn- session-key
  [repo page-uuid]
  [repo page-uuid])

(defn- get-or-create-session!
  [repo page-uuid]
  (let [k (session-key repo page-uuid)]
    (or (get @*sessions k)
        (let [ctx (sci/init {:namespaces {'user default-user-namespace}})]
          (swap! *sessions assoc k ctx)
          ctx))))

(defn restart-session!
  [repo page-uuid]
  (swap! *sessions dissoc (session-key repo page-uuid)))

(defn clear-repo-sessions!
  [repo]
  (swap! *sessions
         (fn [sessions]
           (into {} (remove (fn [[[r _page-uuid] _ctx]] (= r repo))) sessions))))

(defn- eval-code
  [ctx code]
  (try
    {:value (sci/eval-string* ctx code)}
    (catch :default e
      {:error (or (.-message e) (str e))})))

(defn eval-block!
  "Evaluates `code` in the persistent kernel session for `page-uuid`, then
   persists the formatted result onto `block-uuid` as
   :logseq.property.code/result (prefixed with \"Error: \" on failure)."
  [repo page-uuid block-uuid code]
  (if-let [conn (worker-state/get-datascript-conn repo)]
    (let [ctx (get-or-create-session! repo page-uuid)
          {:keys [value error]} (eval-code ctx code)
          result-text (if error (str "Error: " error) (pr-str value))
          block (d/entity @conn [:block/uuid block-uuid])]
      (if block
        (do
          (ldb/transact! conn
                        [[:db/add (:db/id block) :logseq.property.code/result result-text]
                         [:db/add (:db/id block) :logseq.property.code/executed-at (js/Date.now())]])
          {:result result-text :error? (boolean error)})
        (do
          (log/warn :kernel/eval-missing-block {:repo repo :block-uuid block-uuid})
          {:result nil :error? true})))
    {:result nil :error? true}))

(def-thread-api :thread-api/kernel-eval!
  [repo page-uuid block-uuid code]
  (eval-block! repo page-uuid block-uuid code))

(def-thread-api :thread-api/kernel-restart!
  [repo page-uuid]
  (restart-session! repo page-uuid)
  {:status :restarted})
