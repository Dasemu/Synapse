(ns frontend.worker.handler.render-resource.similar
  "\"Similar to this\" renderer resource, backed by the vector search index."
  (:require [frontend.worker.handler.render-resource.common :as common]
            [frontend.worker.search :as search]
            [frontend.worker.state :as worker-state]))

(def ^:private max-similar-pages 5)

(defn- similar-pages
  [db resource-key {:keys [repo]}]
  (let [block-uuid (second resource-key)]
    (common/entity-by-uuid! db :block-uuid block-uuid)
    (let [vector-index (worker-state/get-vector-index repo)
          results (when vector-index
                    (search/similar-blocks (atom db) vector-index block-uuid
                                           {:limit max-similar-pages}))]
      [#{} (mapv #(select-keys % [:db/id :block/uuid :block/title
                                   :block.temp/unique-title :page? :block/page])
                 (or results []))])))

(def resource-renderers
  {:similar-pages (common/renderer 2 similar-pages)})
