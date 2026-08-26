(ns frontend.worker.handler.caldav-test
  (:require [cljs.test :refer [deftest is testing]]
            [frontend.worker.handler.caldav :as caldav]))

(deftest repeat->rrule-only-translates-the-plus-repeat-type
  (testing "advance-from-scheduled with a daily unit and no explicit interval"
    (is (= "FREQ=DAILY"
           (caldav/repeat->rrule {:repeat-type :logseq.property.repeat/repeat-type.plus
                                  :recur-unit :logseq.property.repeat/recur-unit.day}))))
  (testing "an interval > 1 is included"
    (is (= "FREQ=WEEKLY;INTERVAL=2"
           (caldav/repeat->rrule {:repeat-type :logseq.property.repeat/repeat-type.plus
                                  :recur-unit :logseq.property.repeat/recur-unit.week
                                  :recur-frequency 2}))))
  (testing "dotted-plus (advance from completion) has no RRULE equivalent"
    (is (nil? (caldav/repeat->rrule {:repeat-type :logseq.property.repeat/repeat-type.dotted-plus
                                     :recur-unit :logseq.property.repeat/recur-unit.day}))))
  (testing "double-plus (skip to future) has no RRULE equivalent"
    (is (nil? (caldav/repeat->rrule {:repeat-type :logseq.property.repeat/repeat-type.double-plus
                                     :recur-unit :logseq.property.repeat/recur-unit.day})))))

(deftest rrule->repeat-only-understands-a-bare-freq-and-interval
  (testing "a plain FREQ round-trips to the plus repeat type"
    (is (= {:repeat-type :logseq.property.repeat/repeat-type.plus
           :recur-unit :logseq.property.repeat/recur-unit.month
           :recur-frequency nil}
           (caldav/rrule->repeat "FREQ=MONTHLY"))))
  (testing "FREQ;INTERVAL round-trips with the interval preserved"
    (is (= {:repeat-type :logseq.property.repeat/repeat-type.plus
           :recur-unit :logseq.property.repeat/recur-unit.year
           :recur-frequency 3}
           (caldav/rrule->repeat "FREQ=YEARLY;INTERVAL=3"))))
  (testing "anything with BYDAY/COUNT/UNTIL or another unsupported part is left untranslated"
    (is (nil? (caldav/rrule->repeat "FREQ=WEEKLY;BYDAY=MO,WE,FR")))
    (is (nil? (caldav/rrule->repeat "FREQ=DAILY;COUNT=5")))
    (is (nil? (caldav/rrule->repeat "FREQ=DAILY;UNTIL=20260101T000000Z"))))
  (testing "an unrecognized FREQ value is left untranslated"
    (is (nil? (caldav/rrule->repeat "FREQ=SECONDLY"))))
  (testing "nil/blank input is left untranslated"
    (is (nil? (caldav/rrule->repeat nil)))))

(deftest block->vevent-round-trips-through-parse-ical-component
  (let [ms (.getTime (js/Date. "2026-03-15T10:00:00Z"))
        ical (caldav/block->vevent {:uid "evt-1"
                                    :summary "Team sync"
                                    :description "Weekly ; check-in, notes\nwith a line break"
                                    :location "Room A"
                                    :dtstart ms
                                    :dtend (+ ms 3600000)
                                    :rrule "FREQ=WEEKLY"})
        parsed (caldav/parse-ical-component ical)]
    (is (= "VEVENT" (:type parsed)))
    (is (= "evt-1" (:uid parsed)))
    (is (= "Team sync" (:summary parsed)))
    (is (= "Weekly ; check-in, notes\nwith a line break" (:description parsed)))
    (is (= "Room A" (:location parsed)))
    (is (= ms (:dtstart parsed)))
    (is (= (+ ms 3600000) (:dtend parsed)))
    (is (= "FREQ=WEEKLY" (:rrule parsed)))))

(deftest block->vtodo-round-trips-through-parse-ical-component
  (let [ms (.getTime (js/Date. "2026-03-15T10:00:00Z"))
        ical (caldav/block->vtodo {:uid "task-1"
                                   :summary "Ship the release"
                                   :dtstart ms
                                   :due (+ ms 86400000)
                                   :status-ident :logseq.property/status.done})
        parsed (caldav/parse-ical-component ical)]
    (is (= "VTODO" (:type parsed)))
    (is (= "task-1" (:uid parsed)))
    (is (= "Ship the release" (:summary parsed)))
    (is (= ms (:dtstart parsed)))
    (is (= (+ ms 86400000) (:due parsed)))
    (is (= "COMPLETED" (:status parsed)))))

(deftest parse-ical-component-returns-nil-for-non-ical-text
  (is (nil? (caldav/parse-ical-component "not an ics file at all"))))

(deftest contact->vcard-round-trips-through-parse-vcard
  (let [vcard (caldav/contact->vcard {:uid "contact-1"
                                      :full-name "Ada, Lovelace"
                                      :email "ada@example.com"
                                      :phone "+1 555 0100"
                                      :organization "Analytical Engines Ltd"})
        parsed (caldav/parse-vcard vcard)]
    (is (= "contact-1" (:uid parsed)))
    (is (= "Ada, Lovelace" (:full-name parsed)))
    (is (= "ada@example.com" (:email parsed)))
    (is (= "+1 555 0100" (:phone parsed)))
    (is (= "Analytical Engines Ltd" (:organization parsed)))))

(deftest parse-vcard-returns-nil-for-non-vcard-text
  (is (nil? (caldav/parse-vcard "BEGIN:VEVENT\nEND:VEVENT"))))

(def ^:private sample-multistatus
  "<?xml version=\"1.0\"?>
<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">
  <d:response>
    <d:href>/remote.php/dav/calendars/dario/personal/evt-1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>&quot;abc123&quot;</d:getetag>
        <cal:calendar-data>BEGIN:VCALENDAR&#13;
BEGIN:VEVENT&#13;
UID:evt-1&#13;
SUMMARY:Team &amp; sync&#13;
END:VEVENT&#13;
END:VCALENDAR</cal:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/calendars/dario/personal/evt-2.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>&quot;def456&quot;</d:getetag>
        <cal:calendar-data>BEGIN:VCALENDAR&#13;
BEGIN:VEVENT&#13;
UID:evt-2&#13;
SUMMARY:Retro&#13;
END:VEVENT&#13;
END:VCALENDAR</cal:calendar-data>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>")

(deftest parse-multistatus-extracts-href-etag-and-data-per-response
  (let [items (caldav/parse-multistatus "calendar-data" sample-multistatus)]
    (is (= 2 (count items)))
    (is (= "/remote.php/dav/calendars/dario/personal/evt-1.ics" (:href (first items))))
    (is (= "\"abc123\"" (:etag (first items))))
    (is (= "evt-1" (:uid (caldav/parse-ical-component (:data (first items))))))
    (is (= "Team & sync" (:summary (caldav/parse-ical-component (:data (first items))))))
    (is (= "evt-2" (:uid (caldav/parse-ical-component (:data (second items))))))))
