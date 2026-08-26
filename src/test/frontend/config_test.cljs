(ns frontend.config-test
  (:require [cljs.test :refer [deftest is testing]]
            [frontend.config :as config]
            [frontend.state :as state]
            [logseq.common.config :as common-config]))

(deftest get-local-dir-uses-encoded-directory-name
  (with-redefs [state/get-state (fn [] {:system/info {:home-dir "/tmp/home"}})]
    (is (= "/tmp/home/logseq/graphs/foo~2Fbar"
           (config/get-local-dir (str common-config/db-version-prefix "foo/bar"))))))

(deftest get-electron-backup-dir-uses-unified-backup-directory
  (with-redefs [state/get-state (fn [] {:system/info {:home-dir "/tmp/home"}})]
    (is (= "/tmp/home/logseq/graphs/foo~2Fbar/backup"
           (config/get-electron-backup-dir (str common-config/db-version-prefix "foo/bar"))))))

(deftest valid-sync-server-url?-test
  (testing "accepts http and https URLs"
    (is (config/valid-sync-server-url? "https://my-server.example.com"))
    (is (config/valid-sync-server-url? "http://localhost:8787")))

  (testing "rejects non-URL strings"
    (is (not (config/valid-sync-server-url? "not a url")))
    (is (not (config/valid-sync-server-url? "")))
    (is (not (config/valid-sync-server-url? nil)))))
