(ns frontend.components.similar-pages
  "\"Similar pages\" section shown on a page, backed by the vector search
  index's nearest neighbors to the page's own embedding."
  (:require [frontend.components.block :as block]
            [frontend.context.i18n :refer [t]]
            [frontend.db.hooks :as db-hooks]
            [io.factorhouse.hsx.core :as hsx]))

(hsx/defc similar-pages
  [page-uuid]
  (let [results (db-hooks/use-resource [:similar-pages page-uuid])]
    (when (seq results)
      [:div.similar-pages.fade-in.delay {:key "page-similar-pages"}
       [:div.text-sm.font-medium.text-muted-foreground.mb-2 (t :page/similar-pages)]
       [:div.flex.flex-col.gap-1
        (for [{:block/keys [uuid] :as result} results]
          ^{:key (str uuid)}
          [:div.similar-page-item
           (block/page-cp {} result)])]])))
