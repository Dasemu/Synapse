(ns frontend.components.live-note
  "Displays fetched content for a page tagged :logseq.class/LiveNote, backed
  by the :live-note-content render-resource (worker/handler/live_notes.cljs
  fetches and transacts the content on a refresh schedule; this component
  only reads it back reactively)."
  (:require [frontend.context.i18n :refer [t]]
            [frontend.db.hooks :as db-hooks]
            [frontend.handler.notification :as notification]
            [frontend.state :as state]
            [io.factorhouse.hsx.core :as hsx]
            [logseq.shui.hooks :as hooks]))

(defn- format-timestamp
  [ms]
  (when (number? ms)
    (.toLocaleString (js/Date. ms))))

(hsx/defc live-note
  [page-uuid]
  (let [{:keys [url content last-fetched-at last-error]} (db-hooks/use-resource [:live-note-content page-uuid])
        [refreshing? set-refreshing!] (hooks/use-state false)]
    (when url
      [:div.live-note.fade-in.delay {:key "page-live-note"}
       [:div.flex.items-center.justify-between.mb-2
        [:div.text-sm.font-medium.text-muted-foreground (t :live-note/title)]
        [:button.text-sm.text-muted-foreground.hover:text-foreground
         {:disabled refreshing?
          :on-click (fn []
                      (set-refreshing! true)
                      (-> (state/<invoke-db-worker :thread-api/live-note-refresh-now!
                                                   (state/get-current-repo) page-uuid)
                          (.finally (fn []
                                     (set-refreshing! false)
                                     (when last-error
                                       (notification/show! last-error :error))))))}
         (if refreshing? (t :live-note/refreshing) (t :live-note/refresh-now))]]
       (when last-error
         [:div.text-sm.text-error.mb-2 last-error])
       (if content
         [:div.live-note-content.whitespace-pre-wrap.text-sm content]
         [:div.text-sm.text-muted-foreground (t :live-note/not-fetched-yet)])
       (when last-fetched-at
         [:div.text-xs.text-muted-foreground.mt-2
          (str (t :live-note/last-fetched) " " (format-timestamp last-fetched-at))])])))
