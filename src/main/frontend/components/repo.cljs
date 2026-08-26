(ns frontend.components.repo
  (:require [clojure.string :as string]
            [frontend.config :as config]
            [frontend.context.i18n :as i18n :refer [t]]
            [frontend.handler.graph :as graph]
            [frontend.handler.notification :as notification]
            [frontend.handler.repo :as repo-handler]
            [frontend.handler.route :as route-handler]
            [frontend.rfx :as rfx]
            [frontend.state :as state]
            [frontend.ui :as ui]
            [frontend.util :as util]
            [frontend.util.text :as text-util]
            [goog.object :as gobj]
            [logseq.common.util :as common-util]
            [logseq.shui.hooks :as hooks]
            [logseq.shui.ui :as shui]
            [medley.core :as medley]
            [promesa.core :as p]
            [io.factorhouse.hsx.core :as hsx]))

(defn- repo-display-name
  [repo-url]
  (let [repo-name (text-util/get-graph-name-from-path repo-url)]
    (if (config/db-based-graph? repo-name)
      (config/db-graph-name repo-name)
      repo-name)))

(hsx/defc normalized-graph-label
  [{:keys [url] :as graph} on-click]
  (when graph
    [:span.flex.items-center
     (let [local-dir (config/get-local-dir url)
           graph-name (text-util/get-graph-name-from-path url)]
       [:a.flex.items-center {:title local-dir
                              :on-click #(on-click graph)}
        [:span graph-name]])]))

(defn sort-repos-with-metadata-local
  [repos]
  (if-let [m (and (seq repos) (graph/get-metadata-local))]
    (->> repos
         (map (fn [r] (merge r (get m (:url r)))))
         (sort (fn [r1 r2]
                 (compare (or (:last-seen-at r2) (:created-at r2))
                          (or (:last-seen-at r1) (:created-at r1))))))
    repos))

(defn- safe-locale-date
  [dst]
  (when (number? dst)
    (try
      (i18n/locale-format-date (js/Date. dst))
      (catch js/Error _e nil))))

(defn- open-repo-folder!
  [{:keys [root]}]
  (util/open-url (str "file://" root)))

(defn- can-delete-local-graph?
  [repo]
  (repo-handler/removable-repo? repo (state/get-repos)))

(defn- delete-local-graph!
  [{:keys [url] :as repo}]
  (when (can-delete-local-graph? repo)
    (let [graph-name (config/db-graph-name url)
          dialog-config {:cancel-label (t :ui/cancel)
                         :ok-label (t :ui/confirm)}]
      (-> (shui/dialog-confirm!
           (assoc dialog-config
                  :title (t :graph/delete-local-confirm-desc graph-name)
                  :description (t :graph/delete-warning)))
          (p/then (fn []
                    (repo-handler/remove-repo! repo)))))))

(defn graph-open-new-window-target
  [{:keys [url] :as repo}]
  (when-let [graph-id (:graph-id (graph/repo-summary->registry-entry repo))]
    {:repo url
     :graph-id graph-id}))

(defn- <graph-open-new-window-target
  [{:keys [url] :as repo}]
  (if-let [target (graph-open-new-window-target repo)]
    (p/resolved target)
    (p/let [registry (graph/<get-graph-registry)
            entry (graph/resolve-registry-target registry {:graph-identifier url})]
      (when entry
        {:repo (:repo entry)
         :graph-id (:graph-id entry)}))))

(defn open-in-another-tab-action?
  [{:keys [root]}]
  (boolean (and util/web-platform? root)))

(defn open-graph-in-another-tab!
  [repo]
  (if-let [target (graph-open-new-window-target repo)]
    (state/pub-event! [:graph/open-new-window target])
    (p/let [target (<graph-open-new-window-target repo)]
      (when target
        (state/pub-event! [:graph/open-new-window target])))))

(hsx/defc repos-inner
  "Graph list in `All graphs` page"
  [repos]
  (for [{:keys [root url created-at last-seen-at] :as repo}
        (sort-repos-with-metadata-local repos)]
    [:div.flex.justify-between.mb-2.items-center.group {:key url
                                                        "data-testid" url}
     [:div
      [:span.flex.items-center.gap-1
       (normalized-graph-label repo
                               (fn []
                                 (when root
                                   (state/pub-event! [:graph/switch url]))))]
      (when-let [time (some-> (or last-seen-at created-at) (safe-locale-date))]
        [:small.text-muted-foreground (t :graph/last-opened-at-label time)])]

     [:div.controls
      [:div.flex.flex-row.items-center
       (when (and (util/electron?) root)
         [:a.text-xs.items-center.text-gray-08.hover:underline.hidden.group-hover:flex
          {:on-click #(open-repo-folder! repo)}
          (shui/tabler-icon "folder-pin") [:span.pl-1 root]])

       (shui/dropdown-menu
        (shui/dropdown-menu-trigger
         {:asChild true}
         (shui/button
          {:variant "ghost"
           :class "graph-action-btn !px-1"
           :size :sm}
          (ui/icon "dots" {:size 15})))
        (shui/dropdown-menu-content
         {:align "end"}
         (when (open-in-another-tab-action? repo)
           (shui/dropdown-menu-item
            {:key "open-in-another-tab"
             :class "open-in-another-tab-menu-item"
             :on-click #(open-graph-in-another-tab! repo)}
            (t :graph/open-in-another-tab-action)))

         (when root
           (let [disabled? (not (can-delete-local-graph? repo))]
             (shui/dropdown-menu-item
              {:key "delete-locally"
               :class "delete-local-graph-menu-item"
               :disabled disabled?
               :on-click #(when-not disabled?
                            (delete-local-graph! repo))}
              (t :graph/delete-local-action))))))]]]))

(hsx/defc repos-cp
  []
  (let [repos (rfx/use-sub [:me :repos])
        repos (util/distinct-by :url repos)
        repos (remove #(= (:url %) config/demo-repo) repos)
        repos (filter (fn [item]
                        ;; use `config/db-based-graph?` to avoid loading old file graphs
                        (config/db-based-graph? (:url item)))
                      repos)]
    [:div#graphs
     (when-not (util/capacitor?)
       [:h1.title (t :graph/all-graphs)])

     [:div.pl-1.content
      {:class (when-not (util/mobile?) "mt-8")}
      (when-not (util/mobile?)
        [:div.flex.flex-row.my-8
         [:div.mr-8
          (ui/button
           (t :graph/create-new)
           :on-click #(state/pub-event! [:graph/new-db-graph]))]])

      [:div
       [:h2.text-lg.font-medium.mb-4 (t :graph/local-graphs)]
       (when (seq repos)
         (repos-inner repos))]]]))

(defn- repos-dropdown-links [repos current-repo & {:as opts}]
  (let [switch-repos (if-not (nil? current-repo)
                       (remove (fn [repo] (= current-repo (:url repo))) repos) repos) ; exclude current repo
        repo-links (mapv
                    (fn [{:keys [url] :as graph}]
                      (let [repo-url url
                            short-repo-name (text-util/get-graph-name-from-path repo-url)]
                        (when short-repo-name
                          {:title [:span.flex.items-center.title-wrap short-repo-name]
                           :hover-detail repo-url ;; show full path on hover
                           :options {:on-click
                                     (fn [e]
                                       (when-let [on-click (:on-click opts)]
                                         (on-click e))
                                       (if (and (gobj/get e "shiftKey") (:root graph))
                                         (if (util/electron?)
                                           (state/pub-event! [:graph/open-new-window url])
                                           (if-let [target (graph-open-new-window-target graph)]
                                             (state/pub-event! [:graph/open-new-window target])
                                             (p/let [target (<graph-open-new-window-target graph)]
                                               (when target
                                                 (state/pub-event! [:graph/open-new-window target])))))
                                         (state/pub-event! [:graph/switch url])))}})))
                    switch-repos)]
    (->> repo-links (remove nil?))))

(defn- repos-footer []
  [:div.cp__repos-quick-actions
   {:on-click #(shui/popup-hide!)}

   (when-not config/publishing?
     (shui/button
      {:size :sm :variant :ghost
       :on-click #(state/pub-event! [:graph/new-db-graph])}
      (shui/tabler-icon "database-plus")
      [:span (if util/electron? (t :graph/create-db) (t :graph/create-new))]))

   (when-not config/publishing?
     (shui/button
      {:size :sm :variant :ghost
       :on-click (fn [] (route-handler/redirect! {:to :import}))}
      (shui/tabler-icon "database-import")
      [:span (t :import/notes)]))

   (when-not config/publishing?
     (shui/button {:size :sm :variant :ghost
                   :on-click (fn []
                               (if (util/capacitor?)
                                 (state/pub-event! [:mobile/set-tab "graphs"])
                                 (route-handler/redirect-to-all-graphs)))}
                  (shui/tabler-icon "layout-2") [:span (t :graph/all-graphs)]))])

(defn close-sidebar-after-repo-popup-action!
  []
  (when (util/sm-breakpoint?)
    (js/setTimeout #(state/set-left-sidebar-open! false) 0)))

(defn repo-popup-action-target?
  [target]
  (boolean (.closest target "a, button, [role='menuitem']")))

(hsx/defc repos-dropdown-content
  [& {:keys [contentid footer?] :as opts
      :or {footer? true}}]
  (let [current-repo (rfx/use-sub [:git/current-repo])
        repos (rfx/use-sub [:me :repos])
        repos (sort-repos-with-metadata-local repos)
        repos (util/distinct-by :url repos)
        items-fn #(repos-dropdown-links repos current-repo opts)
        header-fn #(when (> (count repos) 1) ; show switch to if there are multiple repos
                     [:div.font-medium.md:text-sm.md:opacity-50.p-2.flex.flex-row.justify-between.items-center
                      [:h4.pb-1 (t :graph.switch/prompt)]])]

    [:div
     {:class (when (<= (count repos) 1) "no-repos")}
     (header-fn)
     [:div.cp__repos-list-wrap
      (for [[idx {:keys [hr item hover-detail title options icon]}] (map-indexed vector (items-fn))]
        (let [on-click' (:on-click options)
              href' (:href options)
              menu-item (if (util/mobile?) ui/menu-link shui/dropdown-menu-item)]
          (if hr
            (if (util/mobile?)
              [:hr.py-2 {:key (str "separator-" idx)}]
              (shui/dropdown-menu-separator {:key (str "separator-" idx)}))
            (menu-item
             (assoc options
                    :key (or href' hover-detail (str "repo-" idx))
                    :title hover-detail
                    :on-click (fn [^js e]
                                (when on-click'
                                  (when-not (false? (on-click' e))
                                    (shui/popup-hide! contentid)))))
             (or item
                 (if href'
                   [:a.flex.items-center.w-full
                    {:href href' :on-click #(shui/popup-hide! contentid)
                     :style {:color "inherit"}} title]
                   [:span.flex.items-center.gap-1.w-full
                    icon [:div title]]))))))]
     (when footer?
       (repos-footer))]))

(defn- current-repo-context-menu-content
  [repo]
  (let [disabled? (not (can-delete-local-graph? repo))]
    [:<>
     (shui/dropdown-menu-item
      {:key "open-repo-folder"
       :on-click #(open-repo-folder! repo)}
      [:span.flex.items-center.gap-1
       (ui/icon "folder-pin")
       (t :graph/open-folder-action)])

     (shui/dropdown-menu-separator)

     (shui/dropdown-menu-item
      {:key "delete-locally"
       :class "delete-local-graph-menu-item"
       :disabled disabled?
       :on-click #(when-not disabled?
                    (delete-local-graph! repo))}
      [:span.flex.items-center.gap-1.text-red-700
       (ui/icon "trash")
       (t :graph/delete-local-action)])]))

(hsx/defc graphs-selector
  []
  (let [current-repo (rfx/use-sub [:git/current-repo])
        user-repos (rfx/use-sub [:me :repos])
        current-repo' (some->> user-repos (medley/find-first #(= current-repo (:url %))))
        short-repo-name (if current-repo
                          (repo-display-name current-repo)
                          (t :graph.switch/select-prompt))
        selector-opts (cond-> {:on-click (fn [^js e]
                                           (shui/popup-show! (.closest (.-target e) "a")
                                                             (fn [{:keys [id]}]
                                                               (repos-dropdown-content
                                                                {:contentid id}))
                                                             {:as-dropdown? true
                                                              :focus-trigger? false
                                                              :content-props
                                                              {:class "repos-list"
                                                               :on-click
                                                               (fn [^js e]
                                                                 (when (repo-popup-action-target? (.-target e))
                                                                   (close-sidebar-after-repo-popup-action!)))}
                                                              :align :start}))}
                        (and (util/electron?) (:root current-repo'))
                        (assoc :on-context-menu
                               (fn [^js e]
                                 (util/stop e)
                                 (shui/popup-show! e
                                                   (fn [] (current-repo-context-menu-content current-repo'))
                                                   {:as-dropdown? true
                                                    :content-props {:on-click (fn [] (shui/popup-hide!))
                                                                    :class "w-60"}}))))]
    [:div.cp__graphs-selector.flex.items-center.justify-between
     [:a.item.flex.items-center.gap-1.select-none
      selector-opts
      [:span.thumb
       (shui/tabler-icon "topology-star" {:size 16})]
      [:strong short-repo-name]
      (shui/tabler-icon "selector" {:size 18})]]))

;; Update invalid-graph-name-warning if characters change
(def multiplatform-reserved-chars ":\\*\\?\"<>|\\#\\\\")

(def reserved-chars-pattern
  (re-pattern (str "[" multiplatform-reserved-chars "]+")))

(defn include-reserved-chars?
  "Includes reserved characters that would broken FS"
  [s]
  (common-util/safe-re-find reserved-chars-pattern s))

(defn- reserved-character-item
  [character label]
  [:li (str character " (" label ")")])

(defn invalid-graph-name-warning
  []
  (notification/show!
   [:div
    [:p (t :graph.validation/name-reserved-characters-warning)]
    [:ul
     (reserved-character-item "<" (t :graph.validation/reserved-character-less-than))
     (reserved-character-item ">" (t :graph.validation/reserved-character-greater-than))
     (reserved-character-item ":" (t :graph.validation/reserved-character-colon))
     (reserved-character-item "\"" (t :graph.validation/reserved-character-double-quote))
     (reserved-character-item "/" (t :graph.validation/reserved-character-forward-slash))
     (reserved-character-item "\\" (t :graph.validation/reserved-character-backslash))
     (reserved-character-item "|" (t :graph.validation/reserved-character-pipe))
     (reserved-character-item "?" (t :graph.validation/reserved-character-question-mark))
     (reserved-character-item "*" (t :graph.validation/reserved-character-asterisk))
     (reserved-character-item "#" (t :graph.validation/reserved-character-hash))
      ;; `+` is used to encode path that includes `:` or `/`
     (reserved-character-item "+" (t :graph.validation/reserved-character-plus))]]
   :warning false))

(defn invalid-graph-name?
  "Returns boolean indicating if DB graph name is invalid. Must be kept in sync with invalid-graph-name-warning"
  [graph-name]
  (or (include-reserved-chars? graph-name)
      (string/includes? graph-name "+")
      (string/includes? graph-name "/")))

(hsx/defc new-db-graph-inner
  []
  (let [[creating-db? set-creating-db?] (hooks/use-state false)
        input-ref (hooks/create-ref)
        new-db-f (fn new-db-f
                   [graph-name]
                   (when-not (or (string/blank? graph-name)
                                 creating-db?)
                     (if (invalid-graph-name? graph-name)
                       (invalid-graph-name-warning)
                       (do
                         (set-creating-db? true)
                         (p/let [_repo (repo-handler/new-db! graph-name)]
                           (shui/dialog-close!))))))
        submit! (fn submit!
                  [^js e click?]
                  (when-let [value (and (or click? (= (gobj/get e "key") "Enter"))
                                        (util/trim-safe (.-value (hooks/deref input-ref))))]
                    (new-db-f value)))]
    (hooks/use-effect!
     (fn []
       (when-let [^js input (hooks/deref input-ref)]
         (js/setTimeout #(.focus input) 32)))
     [])

    [:div.new-graph.flex.flex-col.gap-4.p-1.pt-2
     (shui/input
      {:disabled creating-db?
       :ref input-ref
       :placeholder (t :graph/name-placeholder)
       :on-key-down submit!
       :autoComplete "off"})
     (shui/button
      {:disabled creating-db?
       :on-click #(submit! % true)
       :on-key-down submit!}
      (if creating-db?
        (ui/loading (t :graph/creating))
        (t :ui/submit)))]))

(hsx/defc new-db-graph
  []
  (new-db-graph-inner))
