(ns frontend.worker.handler.render-resource.live-note
  "Live note content renderer resource. The content itself is fetched and
   transacted by frontend.worker.handler.live-notes; this renderer only
   reads it back out and declares the watch key that makes it reactive --
   see render_affected_keys.cljs's generic per-entity [:entity uuid] key,
   emitted whenever any of that entity's attributes change in a tx."
  (:require [frontend.worker.handler.render-resource.common :as common]))

(defn- live-note-content
  [db resource-key _runtime]
  (let [block-uuid (second resource-key)
        page (common/entity-by-uuid! db :block-uuid block-uuid)]
    [#{[:entity block-uuid]}
     {:url (:logseq.property.livenote/url page)
      :content (:logseq.property.livenote/content page)
      :last-fetched-at (:logseq.property.livenote/last-fetched-at page)
      :last-error (:logseq.property.livenote/last-error page)}]))

(def resource-renderers
  {:live-note-content (common/renderer 2 live-note-content)})
