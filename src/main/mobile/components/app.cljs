(ns mobile.components.app
  "App root"
  (:require ["../externals.js"]
            [frontend.config :as config]
            [frontend.components.journal :as journal]
            [frontend.components.quick-add :as quick-add]
            [frontend.context.i18n :as i18n :refer [t]]
            [frontend.handler.common :as common-handler]
            [frontend.handler.editor :as editor-handler]
            [frontend.handler.repo :as repo-handler]
            [frontend.extensions.fsrs :as fsrs]
            [frontend.mobile.util :as mobile-util]
            [frontend.rfx :as rfx]
            [frontend.state :as state]
            [frontend.ui :as ui]
            [frontend.util :as util]
            [frontend.util.text :as text-util]
            [logseq.shui.dialog.core :as shui-dialog]
            [logseq.shui.hooks :as hooks]
            [logseq.shui.popup.core :as shui-popup]
            [logseq.shui.toaster.core :as shui-toaster]
            [logseq.shui.ui :as shui]
            [mobile.bottom-tabs :as bottom-tabs]
            [mobile.components.editor-toolbar :as editor-toolbar]
            [mobile.components.favorites :as favorites]
            [mobile.components.graphs :as graphs]
            [mobile.components.header :as mobile-header]
            [mobile.components.popup :as popup]
            [mobile.components.selection-toolbar :as selection-toolbar]
            [mobile.state :as mobile-state]
            [promesa.core :as p]
            [io.factorhouse.hsx.core :as hsx]))

(hsx/defc component-with-restoring
  [component]
  (let [db-restoring? (rfx/use-sub [:db/restoring?])]
    (if db-restoring?
      [:div.space-y-2.mt-8.mx-0.opacity-75
       (shui/skeleton {:class "h-10 w-full mb-6"})
       (shui/skeleton {:class "h-6 w-full"})
       (shui/skeleton {:class "h-6 w-full"})]
      component)))

(hsx/defc home
  []
  (hooks/use-effect!
   (fn []
     (ui/inject-document-devices-envs!))
   [])
  (component-with-restoring (journal/all-journals)))

(defn use-theme-effects!
  [current-repo theme]
  (hooks/use-effect!
   (fn []
     (state/sync-system-theme!)
     (ui/setup-system-theme-effect!)
     (let [handler (fn [^js e]
                     (when (:ui/system-theme? (state/get-state))
                       (let [is-dark? (boolean (some-> e .-detail .-isDark))]
                         (state/set-theme-mode! (if is-dark? "dark" "light") true))))]
       (.addEventListener js/window "logseq:native-system-theme-changed" handler)
       #(.removeEventListener js/window "logseq:native-system-theme-changed" handler)))
   [])
  (hooks/use-effect!
   #(let [^js doc js/document.documentElement
          ^js cls (.-classList doc)
          ^js cls-body (.-classList js/document.body)]
      (.setAttribute doc "data-theme" theme)
      (if (= theme "dark")                               ;; for tailwind dark mode
        (do (.add cls "dark")
            (doto cls-body (.remove "light-theme") (.add "dark-theme")))
        (do (.remove cls "dark")
            (doto cls-body (.remove "dark-theme") (.add "light-theme")))))
   [theme])

  (hooks/use-effect!
   (fn []
     (some-> js/window.externalsjs (.settleStatusBar)))
   [current-repo]))

(defn use-screen-size-effects!
  []
  (hooks/use-effect!
   (fn []
     (let [handle-size! (fn []
                          (.setProperty (.-style js/document.body)
                                        "--ls-full-screen-height"
                                        (str js/window.screen.height "px")))]
       (handle-size!)
       (.addEventListener js/window "orientationchange" handle-size!)
       #(.removeEventListener js/window "orientationchange" handle-size!)))
   []))

(defn- safe-locale-date
  [timestamp]
  (when (number? timestamp)
    (try
      (i18n/locale-format-date (js/Date. timestamp))
      (catch js/Error _e nil))))

(defn- native-graph-action
  [id title destructive? message]
  {:id id
   :title title
   :destructive destructive?
   :confirmTitle title
   :confirmMessage message
   :confirmButton (t :ui/confirm)
   :cancelButton (t :ui/cancel)})

(defn- native-graph-actions
  [repos {:keys [root] :as graph}]
  (let [graph-name (config/db-graph-name (:url graph))
        warning (t :graph/delete-warning)
        delete-message (fn [message] (str message "\n\n" warning))
        delete-local? (repo-handler/removable-repo? graph repos)]
    (cond-> []
      (and root delete-local?)
      (conj
       (native-graph-action
        "deleteLocal"
        (t :graph/delete-local-action)
        true
        (delete-message (t :graph/delete-local-confirm-desc graph-name)))))))

(defn- native-graph-item
  [repos {:keys [url root created-at last-seen-at] :as graph}]
  (let [display-name (text-util/get-graph-name-from-path url)
        time (safe-locale-date (or last-seen-at created-at))]
    (when (seq display-name)
      {:id url
       :url url
       :displayName display-name
       :subtitle (when time (t :graph/last-opened-at-label time))
       :local (boolean root)
       :readyForUse true
       :downloading false
       :actions (native-graph-actions repos graph)})))

(defn- native-graph-sections
  [repos]
  (let [repos (->> repos
                   (remove #(= (:url %) config/demo-repo))
                   (filter (fn [item] (config/db-based-graph? (:url item))))
                   (util/distinct-by :url))
        items (vec (keep #(native-graph-item repos %) repos))]
    (if (seq items)
      [{:id "local"
        :title (t :graph/local-graphs)
        :refreshable false
        :graphs items}]
      [])))

(defn- use-native-graphs-effects!
  []
  (let [repos (rfx/use-sub [:me :repos])
        route-match (rfx/use-sub [:route-match])
        _preferred-language (rfx/use-sub [:preferred-language])
        [tab] (hooks/use-atom mobile-state/*tab)
        route-name (get-in route-match [:data :name])
        visible? (and (= tab "graphs")
                      (not (contains? #{:import :export} route-name)))
        sections (native-graph-sections repos)
        labels {:refresh (t :ui/refresh)
                :preparing (t :graph/preparing)
                :downloading (t :graph/downloading)}]
    (hooks/use-effect!
     (fn []
       (bottom-tabs/update-native-graphs! {:labels labels
                                           :sections sections
                                           :visible visible?
                                           :refreshing false})
       nil)
     [labels sections visible?])))

(hsx/defc native-graphs-bridge
  []
  (use-native-graphs-effects!)
  [:<>])

(hsx/defc capture
  []
  (hooks/use-effect!
   (fn []
     (p/do!
      (editor-handler/quick-add-ensure-new-block-exists!)
      (when (mobile-util/native-ios?)
        ;; FIXME: android doesn't open keyboard automatically
        (editor-handler/quick-add-open-last-block!))))
   [])
  (quick-add/quick-add))

(hsx/defc flashcards
  []
  (hooks/use-effect!
   (fn []
     (fsrs/update-due-cards-count))
   [])
  [:div.ls-mobile-flashcards
   (fsrs/cards-view nil {:mobile? true
                         :on-header-change mobile-state/set-flashcards-header!
                         :on-selector-change mobile-state/set-flashcards-selector!})])

(hsx/defc other-page
  [route-view tab route-match]
  (let [page-view? (= (get-in route-match [:data :name]) :page)]
    [:div#main-content-container.pl-3.ls-layer
     {:class (if page-view? "pr-2" "pr-3")}
     (if route-view
       (route-view route-match)
       ;; NOTE: `case` caused IllegalArgumentException: Duplicate case test constant
       (cond
         (= tab "graphs") (when-not (mobile-util/native-ios?)
                            (graphs/page))
         (= tab "go to") (favorites/favorites)
         (= tab "search") nil
         (= tab "flashcards") (component-with-restoring (flashcards))
         (= tab "capture") (component-with-restoring (capture))))]))

(hsx/defc main-content
  [tab route-match]
  (let [view (get-in route-match [:data :view])
        home? (and (= tab "home") (nil? view))
        [quick-add-launched? set-quick-add-launched!] (hooks/use-state
                                                       (= @mobile-state/*app-launch-url
                                                          "logseq://mobile/go/quick-add"))]
    (hooks/use-effect!
     (fn []
       (when (and (= tab "home") quick-add-launched?)
         (set-quick-add-launched! false))
       (fn []))
     [tab])
    ;; Two-layer structure:
    ;; - Journals layer keeps its own scroll container and is always in the DOM.
    ;; - Page/other-tab layer keeps its own independent scroll container.
    ;; Both are absolutely positioned and stacked; we toggle visibility.
    [:div.w-full.relative
     ;; Journals scroll container (keep-alive)
     [:div#app-main-home.pl-4.pr-3.absolute.inset-0
      {:class (when-not home? "invisible pointer-events-none")}
      (when-not quick-add-launched?
        (home))]
     ;; Other pages: search, settings, specific page, etc.
     (when-not home?
       (other-page view tab route-match))]))

(hsx/defc app
  [current-repo route-match]
  (let [[tab] (mobile-state/use-tab)
        preferred-language (rfx/use-sub [:preferred-language])
        theme (rfx/use-sub [:ui/theme])]
    (use-screen-size-effects!)
    (use-theme-effects! current-repo theme)
    (hooks/use-effect!
     (fn []
       (when (mobile-util/native-ios?)
         (.requestAnimationFrame
          js/window
          (fn []
            (.requestAnimationFrame
             js/window
             (fn []
               (bottom-tabs/mark-tab-content-ready! tab))))))
       nil)
     [tab route-match])
    (hooks/use-effect!
     (fn []
       (when-let [element (util/app-scroll-container-node)]
         (common-handler/listen-to-scroll! element)))
     [])
    (hooks/use-effect!
     (fn []
       (when (mobile-util/native-platform?)
         (bottom-tabs/configure)))
     [preferred-language])
    [:<>
     (mobile-header/header current-repo tab)
     (main-content tab route-match)]))

(defonce hidden-input
  [:input
   {:id mobile-util/mobile-keyboard-anchor-id
    :type "text"}])

(hsx/defc main
  []
  (let [current-repo (rfx/use-sub [:git/current-repo])
        show-action-bar? (rfx/use-sub [:mobile/show-action-bar?])
        [popup-data] (hooks/use-atom mobile-state/*popup-data)
        [popup-presenting?] (hooks/use-atom mobile-state/*popup-presenting?)
        {:keys [open? content-fn]} popup-data
        show-popup? (and open? content-fn)
        route-match (rfx/use-sub [:route-match])]
    [:main#app-container-wrapper.ls-fold-button-on-right
     [:div#app-container {:class (when (and show-popup? popup-presenting?) "invisible")}
      [:div#main-container.flex.flex-1.overflow-x-hidden
       (app current-repo route-match)]]
     (when (mobile-util/native-ios?)
       (native-graphs-bridge))
     (when show-popup?
       [:div.ls-layer {:class (when-not popup-presenting? "invisible")}
        (popup/popup popup-data)])
     (editor-toolbar/mobile-bar)
     (when show-action-bar?
       (selection-toolbar/action-bar))
     (shui-popup/install-popups)
     (shui-toaster/install-toaster)
     (shui-dialog/install-dialogs)
     [:div.download
      [:a#download.hidden]
      [:a#download-as-transit-debug.hidden]
      [:a#download-as-sqlite-db.hidden]
      [:a#download-as-zip.hidden]]
     hidden-input]))
