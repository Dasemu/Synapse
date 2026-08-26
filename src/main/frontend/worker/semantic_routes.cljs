(ns frontend.worker.semantic-routes
  "Route table and OpenAPI document for the semantic API served by the
   daemon (Fase 9). Ported from deps/db-sync/.../routes/semantic.cljs: that
   version routes by :graph-id under a Cloudflare Worker Durable Object per
   graph. This daemon is bound to a single repo per process (see
   db_worker_node.cljs's `bound-repo`), so the :graph-id path segment and the
   listGraphs operation are dropped -- every request implicitly targets the
   daemon's own bound repo. OAuth2 is likewise dropped in favor of the
   daemon's actual bearer-token session auth (see db-worker-node-auth)."
  (:require [clojure.string :as string]
            [flatland.ordered.map :refer [ordered-map]]
            [reitit.core :as r]))

(def operations
  [{:method "GET" :path "/v1/semantic/pages"
    :handler :semantic/pages-list :operation-id "listPages" :success-status 200}
   {:method "POST" :path "/v1/semantic/pages"
    :handler :semantic/pages-create :operation-id "createPage" :success-status 201}
   {:method "GET" :path "/v1/semantic/pages/:page-id/blocks"
    :handler :semantic/pages-blocks :operation-id "listPageBlocks" :success-status 200}
   {:method "GET" :path "/v1/semantic/pages/:page-id/references"
    :handler :semantic/pages-references :operation-id "listPageReferences" :success-status 200}
   {:method "DELETE" :path "/v1/semantic/pages/:page-id"
    :handler :semantic/pages-delete :operation-id "deletePage" :success-status 204}
   {:method "GET" :path "/v1/semantic/pages/:page-id"
    :handler :semantic/pages-get :operation-id "getPage" :success-status 200}
   {:method "PATCH" :path "/v1/semantic/pages/:page-id"
    :handler :semantic/pages-update :operation-id "updatePage" :success-status 200}
   {:method "GET" :path "/v1/semantic/blocks/:block-id"
    :handler :semantic/blocks-get :operation-id "getBlock" :success-status 200}
   {:method "PATCH" :path "/v1/semantic/blocks/:block-id"
    :handler :semantic/blocks-update :operation-id "updateBlock" :success-status 200}
   {:method "DELETE" :path "/v1/semantic/blocks/:block-id"
    :handler :semantic/blocks-delete :operation-id "deleteBlock" :success-status 204}
   {:method "POST" :path "/v1/semantic/block-moves"
    :handler :semantic/blocks-move :operation-id "moveBlocks" :success-status 200}
   {:method "POST" :path "/v1/semantic/blocks/:block-id/children"
    :handler :semantic/blocks-insert-children :operation-id "insertBlockChildren" :success-status 201}
   {:method "POST" :path "/v1/semantic/block-trees"
    :handler :semantic/blocks-insert-tree :operation-id "insertBlockTree" :success-status 201}
   {:method "PUT" :path "/v1/semantic/blocks/:block-id/properties/:property-id"
    :handler :semantic/blocks-set-property :operation-id "setBlockProperty" :success-status 200}
   {:method "DELETE" :path "/v1/semantic/blocks/:block-id/properties/:property-id"
    :handler :semantic/blocks-delete-property :operation-id "deleteBlockProperty" :success-status 204}
   {:method "POST" :path "/v1/semantic/block-properties/batch-set"
    :handler :semantic/blocks-batch-set-property :operation-id "batchSetBlockProperty" :success-status 200}
   {:method "POST" :path "/v1/semantic/block-properties/batch-delete"
    :handler :semantic/blocks-batch-delete-property :operation-id "batchDeleteBlockProperty" :success-status 200}
   {:method "POST" :path "/v1/semantic/capture"
    :handler :semantic/capture :operation-id "captureToToday" :success-status 201}
   {:method "GET" :path "/v1/semantic/tasks"
    :handler :semantic/tasks-list :operation-id "listTasks" :success-status 200}
   {:method "POST" :path "/v1/semantic/tasks"
    :handler :semantic/tasks-create :operation-id "createTask" :success-status 201}
   {:method "GET" :path "/v1/semantic/tags"
    :handler :semantic/tags-list :operation-id "listTags" :success-status 200}
   {:method "POST" :path "/v1/semantic/tags"
    :handler :semantic/tags-create :operation-id "createTag" :success-status 201}
   {:method "GET" :path "/v1/semantic/tags/:tag-id"
    :handler :semantic/tags-get :operation-id "getTag" :success-status 200}
   {:method "GET" :path "/v1/semantic/tags/:tag-id/objects"
    :handler :semantic/tags-objects :operation-id "listTagObjects" :success-status 200}
   {:method "PATCH" :path "/v1/semantic/tags/:tag-id"
    :handler :semantic/tags-update :operation-id "updateTag" :success-status 200}
   {:method "DELETE" :path "/v1/semantic/tags/:tag-id"
    :handler :semantic/tags-delete :operation-id "deleteTag" :success-status 204}
   {:method "GET" :path "/v1/semantic/properties"
    :handler :semantic/properties-list :operation-id "listProperties" :success-status 200}
   {:method "POST" :path "/v1/semantic/properties"
    :handler :semantic/properties-create :operation-id "createProperty" :success-status 201}
   {:method "GET" :path "/v1/semantic/properties/:property-id"
    :handler :semantic/properties-get :operation-id "getProperty" :success-status 200}
   {:method "PATCH" :path "/v1/semantic/properties/:property-id"
    :handler :semantic/properties-update :operation-id "updateProperty" :success-status 200}
   {:method "DELETE" :path "/v1/semantic/properties/:property-id"
    :handler :semantic/properties-delete :operation-id "deleteProperty" :success-status 204}
   {:method "GET" :path "/v1/semantic/assets"
    :handler :semantic/assets-list :operation-id "listAssets" :success-status 200}
   {:method "POST" :path "/v1/semantic/assets"
    :handler :semantic/assets-create :operation-id "createAsset" :success-status 201}
   {:method "GET" :path "/v1/semantic/assets/:asset-block-id"
    :handler :semantic/assets-get :operation-id "getAsset" :success-status 200}
   {:method "GET" :path "/v1/semantic/search"
    :handler :semantic/search :operation-id "searchGraph" :success-status 200}])

(def operation-docs
  "Public so worker.mcp-server can source MCP tool titles/descriptions from
  the same place the OpenAPI document does, instead of a second copy that
  could drift."
  {"listPages" ["List pages" "Returns a cursor-paginated list of page blocks in the graph."]
   "createPage" ["Create a page" "Creates a page block with the supplied title."]
   "listPageBlocks" ["List a page's blocks" "Returns a cursor-paginated list of the page's top-level blocks, including each selected block's descendant tree."]
   "listPageReferences" ["List references to a page" "Returns a cursor-paginated flat list of blocks and pages that reference the addressed page. Results are not recursively expanded."]
   "getPage" ["Get a page" "Returns one page block by UUID without loading its block tree."]
   "updatePage" ["Update a page" "Renames one page identified by its block UUID."]
   "deletePage" ["Delete a page" "Deletes one page through Logseq's page deletion rules, including its block tree."]
   "getBlock" ["Get a block" "Returns one block or page by UUID without loading an unbounded tree."]
   "updateBlock" ["Update a block" "Replaces the title of one block identified by UUID."]
   "deleteBlock" ["Delete a block" "Deletes one block and its descendants. Asset blocks are deleted through this same operation."]
   "moveBlocks" ["Move blocks" "Moves one or more blocks together before, after, into the first-child position, or into the last-child position of a target block. Logseq preserves the blocks' outliner order."]
   "insertBlockChildren" ["Insert child block trees" "Appends or prepends one or more recursive block trees as children of the addressed block."]
   "insertBlockTree" ["Insert block trees" "Inserts one or more recursive block trees relative to an explicit target block."]
   "setBlockProperty" ["Set a block property" "Idempotently adds or replaces one typed property value on one block in a DB graph. The property path value may be its UUID or ident. Never write legacy file-graph key:: value syntax into the block title."]
   "deleteBlockProperty" ["Delete a block property" "Idempotently removes one property from one block. The property definition itself is not deleted."]
   "batchSetBlockProperty" ["Set block properties in a batch" "Atomically adds or replaces multiple typed block-property values in a DB graph after validating every entry. Property IDs are UUIDs or idents; never use legacy file-graph key:: value syntax."]
   "batchDeleteBlockProperty" ["Delete block properties in a batch" "Atomically removes multiple block-property values after validating every entry."]
   "captureToToday" ["Capture to today's journal" "Creates today's journal page when necessary and appends the supplied recursive block trees to its end."]
   "listTasks" ["List DB tasks" "Lists cursor-paginated DB Task objects using the Task class index, with optional status, priority, and title-content filters. Use this operation instead of graph search to retrieve tasks."]
   "createTask" ["Create a DB task" "Creates a DB Task object with the Task class and typed status, priority, scheduled, and deadline properties. Never encode a task as Markdown TODO text."]
   "listTags" ["List tags" "Returns a cursor-paginated list of tag entities in the graph."]
   "createTag" ["Create a tag" "Creates a Logseq tag with the supplied title."]
   "getTag" ["Get a tag" "Returns one tag by UUID."]
   "listTagObjects" ["List objects with a tag" "Returns a cursor-paginated flat list of blocks and pages tagged with the addressed tag. Results are not recursively expanded."]
   "updateTag" ["Update a tag" "Renames one tag identified by UUID."]
   "deleteTag" ["Delete a tag" "Deletes one tag through Logseq's page deletion rules."]
   "listProperties" ["List property definitions" "Returns a cursor-paginated list of property definitions in the graph."]
   "createProperty" ["Create a property definition" "Creates a typed property definition in a DB graph with its name, value type, and cardinality. Use the returned UUID or ident to set values; never use legacy file-graph key:: value syntax."]
   "getProperty" ["Get a property definition" "Returns one property definition by UUID."]
   "updateProperty" ["Update a property definition" "Updates the name, value type, or cardinality of one typed property definition in a DB graph. Never use legacy file-graph key:: value syntax."]
   "deleteProperty" ["Delete a property definition" "Deletes one property definition through Logseq's property deletion rules."]
   "listAssets" ["List assets" "Returns a cursor-paginated list of asset blocks with optional creation and update time filters."]
   "createAsset" ["Upload an asset" "Streams a file of at most 100MB to the daemon's local asset storage and creates its Asset-class block. Requires a client-computed SHA-256 checksum."]
   "getAsset" ["Get an asset" "Returns asset metadata and a server-relative URL for the asset. Resolve the URL against the daemon's own base URL; it requires the same bearer token as every other route (or a ?token= query fallback for direct navigation)."]
   "searchGraph" ["Search graph resources" "Searches blocks, tags, properties, and assets by title and returns cursor-paginated results."]})

(defn- operation-routes []
  (->> operations
       (group-by :path)
       (mapv (fn [[path path-operations]]
               [path {:methods (into {} (map (juxt :method identity)) path-operations)}]))))

(def ^:private router
  (r/router (operation-routes)))

(defn match [method path]
  (when-let [route (r/match-by-path router path)]
    (when-let [operation (get-in route [:data :methods method])]
      (merge operation (select-keys route [:path-params])))))

(defn- openapi-path [path]
  (string/replace path #":([^/]+)" "{$1}"))

(defn- path-parameters [path]
  (mapv (fn [[_ parameter]]
          (cond-> {:name parameter :in "path" :required true :schema {:type "string"}}
            (= "property-id" parameter)
            (assoc :description "Property UUID, qualified ident, or exact title.")))
        (re-seq #":([^/]+)" path)))

(def ^:private pagination-parameters
  [{:name "limit" :in "query" :schema {:type "integer" :minimum 1 :maximum 200 :default 50}}
   {:name "cursor" :in "query" :schema {:type "string"}}])

(def ^:private time-filter-parameters
  [{:name "created-after" :in "query"
    :description "Return entities created strictly after this Unix epoch timestamp in milliseconds."
    :schema {:type "integer" :minimum 0}}
   {:name "updated-after" :in "query"
    :description "Return entities updated strictly after this Unix epoch timestamp in milliseconds."
    :schema {:type "integer" :minimum 0}}])

(def ^:private asset-upload-parameters
  [{:name "file-name" :in "query" :required true :schema {:type "string"}}
   {:name "size" :in "query" :required true
    :description "Exact decoded file size in bytes. Uploads larger than 100MB are rejected."
    :schema {:type "integer" :minimum 0 :maximum 104857600}}
   {:name "title" :in "query" :schema {:type "string"}}
   {:name "page-id" :in "query"
    :description "Destination page UUID. Omit to append to today's journal."
    :schema {:type "string"}}
   {:name "checksum" :in "query" :required true
    :description "Lowercase or uppercase SHA-256 hex digest of the file."
    :schema {:type "string" :pattern "^[0-9a-fA-F]{64}$"}}
   {:name "encoding" :in "query"
    :description "Use base64 when the caller cannot transfer raw binary. The server decodes it as a stream before writing to disk; size and checksum describe the decoded file."
    :schema {:type "string" :enum ["base64"]}}])

(defn- request-schema [operation-id]
  (case operation-id
    "createPage" {:required ["title"] :properties {:title {:type "string"}}}
    "updatePage" {:required ["title"] :properties {:title {:type "string"}}}
    "updateBlock" {:required ["title"] :properties {:title {:type "string"}}}
    "moveBlocks" {:required ["block-ids" "target-id" "position"]
                  :properties {:block-ids {:type "array" :minItems 1 :uniqueItems true
                                           :items {:type "string"}}
                               :target-id {:type "string"}
                               :position {:enum ["before" "after" "first-child" "last-child"]}}}
    "insertBlockChildren" {:required ["position" "blocks"]
                           :properties {:position {:enum ["append" "prepend"]}
                                        :blocks {:type "array" :items {:$ref "#/components/schemas/BlockTree"}}}}
    "insertBlockTree" {:required ["target-id" "blocks"]
                       :properties {:target-id {:type "string"}
                                    :position {:enum ["append" "prepend"] :default "append"}
                                    :blocks {:type "array" :items {:$ref "#/components/schemas/BlockTree"}}}}
    "setBlockProperty" {:required ["value"]
                        :properties {:value {:$ref "#/components/schemas/PropertyValue"}}}
    "batchSetBlockProperty" {:required ["entries"]
                             :properties {:isResetExistingValues
                                          {:type "boolean" :default false
                                           :description "For cardinality-many values, replace existing values when true; append when false or omitted."}
                                          :entries {:type "array"
                                                    :items {:type "object" :required ["block-id" "property-id" "value"]
                                                            :properties {:block-id {:type "string"}
                                                                         :property-id {:type "string"}
                                                                         :value {:$ref "#/components/schemas/PropertyValue"}}}}}}
    "batchDeleteBlockProperty" {:required ["entries"]
                                :properties {:entries {:type "array"
                                                       :items {:type "object" :required ["block-id" "property-id"]
                                                               :properties {:block-id {:type "string"}
                                                                            :property-id {:type "string"}}}}}}
    "captureToToday" {:required ["blocks"]
                      :properties {:blocks {:type "array" :items {:$ref "#/components/schemas/BlockTree"}}}}
    "createTask" {:required ["title"]
                  :properties {:title {:type "string"}
                               :page-id {:type "string"
                                         :description "Destination page UUID. Omit to append to today's journal."}
                               :status {:$ref "#/components/schemas/TaskStatusSelector"}
                               :priority {:$ref "#/components/schemas/TaskPrioritySelector"}
                               :scheduled {:type "integer" :description "Scheduled date as YYYYMMDD."}
                               :deadline {:type "integer" :description "Deadline date as YYYYMMDD."}}}
    "createTag" {:required ["title"] :properties {:title {:type "string"}}}
    "updateTag" {:required ["title"] :properties {:title {:type "string"}}}
    "createProperty" {:required ["title"]
                      :properties {:title {:type "string"} :type {:$ref "#/components/schemas/PropertyType"}
                                   :cardinality {:enum ["db.cardinality/one" "db.cardinality/many"]}}}
    "updateProperty" {:properties {:title {:type "string"} :type {:$ref "#/components/schemas/PropertyType"}
                                    :cardinality {:enum ["db.cardinality/one" "db.cardinality/many"]}}}
    nil))

(defn- operation-parameters [{:keys [operation-id path]}]
  (cond-> (path-parameters path)
    (contains? #{"listPages" "listPageBlocks" "listPageReferences" "listTasks" "listTags"
                 "listTagObjects" "listProperties" "listAssets" "searchGraph"} operation-id)
    (into pagination-parameters)
    (contains? #{"listPages" "listTasks" "listTags" "listTagObjects" "listProperties" "listAssets" "searchGraph"}
               operation-id)
    (into time-filter-parameters)
    (= "createAsset" operation-id)
    (into asset-upload-parameters)
    (= "searchGraph" operation-id)
    (into [{:name "q" :in "query" :required true :schema {:type "string"}}
           {:name "types" :in "query" :schema {:type "string"}}])
    (= "listTasks" operation-id)
    (into [{:name "status" :in "query" :schema {:$ref "#/components/schemas/TaskStatusSelector"}}
           {:name "priority" :in "query" :schema {:$ref "#/components/schemas/TaskPrioritySelector"}}
           {:name "content" :in "query" :schema {:type "string"}
            :description "Case-insensitive task title filter."}])))

(defn- request-body [operation-id]
  (if (= "createAsset" operation-id)
    {:required true
     :content {"application/octet-stream"
               {:schema {:type "string" :format "binary"}}}}
    (when-let [schema (request-schema operation-id)]
      {:required true
       :content {"application/json" {:schema (assoc schema :type "object")}}})))

(defn- list-response [response-key schema description]
  {:description description
   :content {"application/json"
             {:schema {:type "object"
                       :required [(name response-key)]
                       :properties {response-key {:type "array"
                                                  :items {:$ref (str "#/components/schemas/" schema)}}
                                    :next-cursor {:type "string"}}}}}})

(defn- entity-response [schema description]
  {:description description
   :content {"application/json" {:schema {:$ref (str "#/components/schemas/" schema)}}}})

(defn- operation-responses [operation-id]
  (merge {"200" (case operation-id
                  "listPages" (list-response :pages "PageResponse" "A cursor page of pages")
                  "listPageBlocks"
                  (list-response :blocks "BlockResponse" "A cursor page of top-level block trees")
                  "listPageReferences" (list-response :references "BlockResponse" "A cursor page of page references")
                  "listTasks"
                  (list-response :tasks "TaskResponse" "A cursor page of DB Task objects")
                  "listTags" (list-response :tags "TagResponse" "A cursor page of tags")
                  "listTagObjects" (list-response :objects "BlockResponse" "A cursor page of tagged objects")
                  "listProperties" (list-response :properties "PropertyResponse" "A cursor page of property definitions")
                  "listAssets"
                  (list-response :assets "AssetResponse" "A cursor page of asset blocks")
                  "searchGraph" (list-response :results "SearchResultResponse" "A cursor page of search results")
                  "getPage" (entity-response "PageResponse" "A page")
                  "updatePage" (entity-response "PageResponse" "The updated page")
                  "getBlock" (entity-response "BlockResponse" "A block")
                  "updateBlock" (entity-response "BlockResponse" "The updated block")
                  "getTag" (entity-response "TagResponse" "A tag")
                  "updateTag" (entity-response "TagResponse" "The updated tag")
                  "getProperty"
                  (entity-response "PropertyResponse" "A property definition and its user-extensible choices")
                  "updateProperty" (entity-response "PropertyResponse" "The updated property definition")
                  "getAsset" (entity-response "AssetResponse" "Asset metadata and a server-relative URL")
                  {:description "Success"})
          "400" {:description "Invalid request"}
          "401" {:description "Missing or invalid bearer token"}
          "404" {:description "Not found"}}
         (case operation-id
           "createPage" {"201" (entity-response "PageResponse" "The created page")}
           "insertBlockChildren" {"201" {:description "The inserted block trees"}}
           "insertBlockTree" {"201" {:description "The inserted block trees"}}
           "createTag" {"201" (entity-response "TagResponse" "The created tag")}
           "createProperty" {"201" (entity-response "PropertyResponse" "The created property definition")}
           "createTask" {"201" (entity-response "TaskResponse" "The created DB Task object")}
           "createAsset" {"201" (entity-response "AssetResponse" "The created asset block")}
           {})))

(defn openapi-document
  [{:keys [repo]}]
  {:openapi "3.1.0"
   :info {:title "Logseq Daemon Semantic API" :version "1.0.0"
          :description (str "Operates on the graph bound to this daemon instance"
                             (when repo (str " (" repo ")")) ".")}
   :servers [{:url "/"}]
   :components
   {:schemas (ordered-map
              :BlockTree {:type "object" :required ["title"]
                          :properties {:title {:type "string"}
                                       :children {:type "array" :items {:$ref "#/components/schemas/BlockTree"}}}}
              :BlockResponse {:type "object" :required ["uuid" "kind" "title"]
                              :properties {:uuid {:type "string"}
                                           :kind {:type "string"}
                                           :title {:type "string"}
                                           :order {:type "string"}
                                           :parent-id {:type "string"}
                                           :page-id {:type "string"}
                                           :children {:type "array"
                                                      :items {:$ref "#/components/schemas/BlockResponse"}}}}
              :PageResponse {:allOf [{:$ref "#/components/schemas/BlockResponse"}]}
              :TagResponse {:type "object" :required ["uuid" "title"]
                            :properties {:uuid {:type "string"}
                                         :title {:type "string"}
                                         :ident {:type "string"}}}
              :SearchResultResponse {:type "object" :required ["uuid" "title" "resource"]
                                     :properties {:uuid {:type "string"}
                                                  :title {:type "string"}
                                                  :resource {:enum ["blocks" "tags" "properties" "assets"]}}}
              :EntitySelector {:type "string"
                               :description "An existing entity UUID, qualified ident, or exact title."}
              :TaskStatusSelector
              {:description "A built-in status alias or any user-extensible Status property choice selected by UUID, ident, or exact title."
               :anyOf [{:type "string" :enum ["todo" "doing" "in-review" "done" "canceled" "backlog"]}
                       {:$ref "#/components/schemas/EntitySelector"}]}
              :TaskPrioritySelector
              {:description "A built-in priority alias or any user-extensible Priority property choice selected by UUID, ident, or exact title."
               :anyOf [{:type "string" :enum ["low" "medium" "high" "urgent"]}
                       {:$ref "#/components/schemas/EntitySelector"}]}
              :PropertyChoice {:type "object" :required ["uuid" "title"]
                               :properties {:uuid {:type "string"}
                                            :ident {:type "string"}
                                            :title {:type "string"}
                                            :value {}}}
              :PropertyResponse {:type "object" :required ["uuid" "title" "type" "cardinality"]
                                 :properties {:uuid {:type "string"}
                                              :ident {:type "string"}
                                              :title {:type "string"}
                                              :type {:type "string"}
                                              :cardinality {:type "string"}
                                              :choices {:type "array"
                                                        :description "The current user-extensible choices for this property."
                                                        :items {:$ref "#/components/schemas/PropertyChoice"}}}}
              :PropertyType {:type "string"
                             :enum ["default" "number" "date" "datetime" "checkbox" "url" "node" "asset"
                                    "map" "json" "string"]}
              :AssetResponse {:type "object" :required ["uuid" "title" "type" "size" "checksum"]
                              :properties {:uuid {:type "string"}
                                           :title {:type "string"}
                                           :type {:type "string"}
                                           :size {:type "integer" :minimum 0 :maximum 104857600}
                                           :checksum {:type "string" :pattern "^[0-9a-f]{64}$"}
                                           :url {:type "string"}}}
              :TaskResponse {:allOf [{:$ref "#/components/schemas/BlockResponse"}
                                     {:type "object"
                                      :required ["status"]
                                      :properties {:status {:$ref "#/components/schemas/PropertyChoice"}
                                                   :priority {:$ref "#/components/schemas/PropertyChoice"}
                                                   :scheduled {:type "integer"}
                                                   :deadline {:type "integer"}}}]}
              :PropertyValue
              {:description "For ref properties, pass an existing entity by UUID, ident, or title. Status and Priority also accept case-insensitive built-in aliases such as TODO, DONE, and HIGH; user-defined choices use UUID, ident, or exact title. Class properties such as Tags accept a class UUID, ident, or title, or an array of those selectors. Scalar properties accept their JSON scalar value."
               :oneOf [{:type "string"} {:type "number"} {:type "boolean"}
                       {:type "array" :items {:oneOf [{:type "string"} {:type "number"}]}}]})
    :securitySchemes
    {:bearerAuth {:type "http" :scheme "bearer"
                  :description "Session token from POST /v1/auth/login."}}}
   :security [{:bearerAuth []}]
   :paths
   (reduce (fn [paths {:keys [method path operation-id]}]
             (let [[summary description] (get operation-docs operation-id)]
               (assoc-in paths [(openapi-path path) (keyword (.toLowerCase method))]
                       (cond-> {:operationId operation-id
                                :summary summary
                                :description description
                                :parameters (operation-parameters {:operation-id operation-id :path path})
                                :responses (operation-responses operation-id)}
                         (request-body operation-id)
                         (assoc :requestBody (request-body operation-id))))))
           (ordered-map)
           operations)})
