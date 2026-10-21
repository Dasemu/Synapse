mod mock;

use chrono::NaiveDate;
use chrono_tz::Europe::Madrid;
use mock::{HOME, Mock, PERSONAL, TASKS};
use synapse_caldav::{
    Config, Credentials, DavClient, events_for_day, materialize_day, refresh_calendars, sync,
};
use synapse_core::{Graph, Placement};

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn ms(y: i32, m: u32, d: u32, h: u32) -> i64 {
    day(y, m, d)
        .and_hms_opt(h, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

fn client(m: &Mock) -> DavClient {
    DavClient::new(
        &m.base,
        Credentials {
            username: "u".into(),
            password: "app-pw".into(),
        },
    )
    .unwrap()
}

fn graph() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    g.set_now(Some(ms(2026, 10, 21, 9)));
    (dir, g)
}

fn vtodo(uid: &str, body: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nBEGIN:VTODO\r\nUID:{uid}\r\n{body}END:VTODO\r\nEND:VCALENDAR\r\n"
    )
}

fn page_blocks(g: &Graph, title: &str) -> Vec<(usize, synapse_core::Block)> {
    let p = g.page_by_title(title).unwrap().unwrap();
    g.page_tree(p.id).unwrap()
}

const STANDUP: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:standup\r\nSUMMARY:Standup\r\nDTSTART;TZID=Europe/Madrid:20261007T100000\r\nDTEND;TZID=Europe/Madrid:20261007T103000\r\nRRULE:FREQ=WEEKLY;BYDAY=WE\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:dentist\r\nSUMMARY:Dentist\r\nDTSTART;TZID=Europe/Madrid:20261021T170000\r\nDURATION:PT45M\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[tokio::test]
async fn discovery_and_calendar_listing() {
    let m = Mock::start().await;
    let (_d, g) = graph();
    let c = client(&m);
    let cals = refresh_calendars(&g, &c).await.unwrap();
    assert_eq!(cals.len(), 2);
    let tasks = cals.iter().find(|c| c.href == TASKS).unwrap();
    assert!(tasks.supports_tasks && !tasks.supports_events);
    let personal = cals.iter().find(|c| c.href == PERSONAL).unwrap();
    assert!(personal.supports_events && personal.supports_tasks);
    assert!(m.count_requests("PROPFIND /.well-known/caldav") >= 1);
    assert_eq!(c.discover().await.unwrap(), HOME);
}

#[tokio::test]
async fn events_recurrence_and_journal_materialisation() {
    let m = Mock::start().await;
    m.set(&format!("{PERSONAL}standup.ics"), STANDUP);
    let (_d, g) = graph();
    let cfg = Config::new(Madrid);
    let rep = sync(&g, &client(&m), &cfg).await.unwrap();
    assert_eq!(rep.events_changed, 1);

    // 2026-10-21 is a Wednesday: the weekly standup and the dentist.
    let evs = events_for_day(&g, day(2026, 10, 21), Madrid).unwrap();
    let names: Vec<_> = evs.iter().map(|e| e.summary.as_str()).collect();
    assert_eq!(names, ["Standup", "Dentist"]);
    assert!(evs[0].recurring && !evs[1].recurring);
    assert_eq!(evs[1].end - evs[1].start, chrono::Duration::minutes(45));
    assert!(
        events_for_day(&g, day(2026, 10, 22), Madrid)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        events_for_day(&g, day(2026, 10, 28), Madrid).unwrap().len(),
        1
    );

    let ids = materialize_day(&g, day(2026, 10, 21), Madrid).unwrap();
    assert_eq!(ids.len(), 2);
    let j = g.journal(day(2026, 10, 21)).unwrap();
    let tree = g.page_tree(j.id).unwrap();
    assert_eq!(tree[0].1.content, "10:00-10:30 [[Standup]]");
    assert_eq!(tree[1].1.content, "17:00-17:45 Dentist");
    // The recurring meeting got its own page (via the [[link]]), the one-off did not.
    assert!(g.page_by_title("Standup").unwrap().is_some());
    assert!(g.page_by_title("Dentist").unwrap().is_none());
    // Idempotent.
    assert_eq!(materialize_day(&g, day(2026, 10, 21), Madrid).unwrap(), ids);
    assert_eq!(g.page_tree(j.id).unwrap().len(), 2);

    // Dentist cancelled remotely: the next sync + materialise removes its block only.
    m.remove(&format!("{PERSONAL}standup.ics"));
    m.set(
        &format!("{PERSONAL}standup.ics"),
        &STANDUP.replace("UID:dentist", "UID:dentist\r\nSTATUS:CANCELLED"),
    );
    sync(&g, &client(&m), &cfg).await.unwrap();
    materialize_day(&g, day(2026, 10, 21), Madrid).unwrap();
    let tree = g.page_tree(j.id).unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].1.content, "10:00-10:30 [[Standup]]");
}

#[tokio::test]
async fn task_round_trip_both_directions() {
    let m = Mock::start().await;
    let path = format!("{TASKS}milk.ics");
    m.set(&path, &vtodo("milk", "SUMMARY:Buy milk\r\nPRIORITY:1\r\nDUE;VALUE=DATE:20261025\r\nX-KEEP:me\r\nLAST-MODIFIED:20261020T080000Z\r\n"));
    let (_d, g) = graph();
    let cfg = Config::new(Madrid);
    let c = client(&m);

    // Remote -> local.
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.tasks_pulled, 1);
    let blocks = page_blocks(&g, "Tasks");
    assert_eq!(blocks.len(), 1);
    let b = &blocks[0].1;
    assert_eq!(b.content, "TODO [#A] Buy milk\nDEADLINE: <2026-10-25 Sun>");
    assert_eq!(b.properties["caldav-uid"], "milk");
    assert_eq!(b.properties["caldav-calendar"], TASKS);

    // Local completion -> remote.
    g.set_now(Some(ms(2026, 10, 21, 10)));
    g.update_block(b.id, "DONE [#A] Buy milk\nDEADLINE: <2026-10-25 Sun>")
        .unwrap();
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.tasks_updated_remote, 1);
    let remote = m.body(&path).unwrap();
    assert!(
        remote.contains("STATUS:COMPLETED") && remote.contains("PERCENT-COMPLETE:100"),
        "{remote}"
    );
    assert!(remote.contains("COMPLETED:20261021T100000Z"), "{remote}");
    assert!(
        remote.contains("X-KEEP:me"),
        "unknown properties survive: {remote}"
    );
    // The PUT was conditional on the etag we had.
    assert!(
        m.state
            .lock()
            .unwrap()
            .puts
            .iter()
            .any(|(p, im)| p == &path && im.is_some())
    );
    // No echo: another sync changes nothing.
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!((rep.tasks_updated_remote, rep.conflicts), (0, 0));

    // Remote reopens it as in-process -> local DOING.
    let reopened = remote
        .replace("STATUS:COMPLETED", "STATUS:IN-PROCESS")
        .replace("PERCENT-COMPLETE:100", "PERCENT-COMPLETE:30")
        .replace("COMPLETED:20261021T100000Z\r\n", "");
    m.set(&path, &reopened);
    sync(&g, &c, &cfg).await.unwrap();
    let b = g.get_block(b.id).unwrap().unwrap();
    assert_eq!(b.content, "DOING [#A] Buy milk\nDEADLINE: <2026-10-25 Sun>");
}

#[tokio::test]
async fn new_journal_task_goes_to_default_list_and_completes_remotely() {
    let m = Mock::start().await;
    m.set(&format!("{PERSONAL}standup.ics"), STANDUP); // gives the account an events-only-ish calendar
    let (_d, g) = graph();
    let mut cfg = Config::new(Madrid);
    cfg.default_task_calendar = Some(TASKS.into());
    let c = client(&m);
    sync(&g, &c, &cfg).await.unwrap(); // establishes "enabled_at"

    let j = g.today_journal().unwrap();
    let plain = g
        .insert_block(j.id, Placement::LastChildOf(None), "just a note")
        .unwrap();
    let t = g
        .insert_block(
            j.id,
            Placement::LastChildOf(None),
            "LATER [#B] Call the bank\nSCHEDULED: <2026-10-22 Thu 09:30>",
        )
        .unwrap();
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.tasks_created_remote, 1);
    let paths = m.paths(TASKS);
    assert_eq!(paths.len(), 1);
    let body = m.body(&paths[0]).unwrap();
    assert!(
        body.contains("SUMMARY:Call the bank") && body.contains("PRIORITY:5"),
        "{body}"
    );
    assert!(
        body.contains("DTSTART;TZID=Europe/Madrid:20261022T093000"),
        "{body}"
    );
    assert!(
        g.get_block(plain.id)
            .unwrap()
            .unwrap()
            .properties
            .is_empty()
    );
    let t = g.get_block(t.id).unwrap().unwrap();
    assert!(t.properties.contains_key("caldav-uid"));
    assert_eq!(
        t.content, "LATER [#B] Call the bank\nSCHEDULED: <2026-10-22 Thu 09:30>",
        "content untouched"
    );

    // Completed in another app -> DONE here; LATER spelling kept only for open states.
    m.set(
        &paths[0],
        &body.replace("STATUS:NEEDS-ACTION", "STATUS:COMPLETED"),
    );
    sync(&g, &c, &cfg).await.unwrap();
    let t = g.get_block(t.id).unwrap().unwrap();
    assert!(
        t.content.starts_with("DONE [#B] Call the bank"),
        "{}",
        t.content
    );
}

#[tokio::test]
async fn conflict_last_modified_wins_and_loser_kept_as_child() {
    let m = Mock::start().await;
    let path = format!("{TASKS}t.ics");
    m.set(
        &path,
        &vtodo(
            "t",
            "SUMMARY:Plan trip\r\nLAST-MODIFIED:20261021T080000Z\r\n",
        ),
    );
    let (_d, g) = graph();
    let cfg = Config::new(Madrid);
    let c = client(&m);
    sync(&g, &c, &cfg).await.unwrap();
    let b = page_blocks(&g, "Tasks")[0].1.clone();

    // Case 1: remote edited later (11:00) than the local edit (10:00): remote wins.
    g.set_now(Some(ms(2026, 10, 21, 10)));
    g.update_block(b.id, "TODO Plan trip to Rome").unwrap();
    m.set(
        &path,
        &vtodo(
            "t",
            "SUMMARY:Plan trip to Paris\r\nLAST-MODIFIED:20261021T110000Z\r\n",
        ),
    );
    g.set_now(Some(ms(2026, 10, 21, 12)));
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.conflicts, 1);
    let tree = page_blocks(&g, "Tasks");
    assert_eq!(tree[0].1.content, "TODO Plan trip to Paris");
    assert_eq!(tree[1].0, 1);
    assert!(tree[1].1.content.contains("local") && tree[1].1.content.contains("Plan trip to Rome"));

    // Case 2: local edited later (14:00) than the remote (13:00): local wins and is pushed.
    m.set(
        &path,
        &vtodo(
            "t",
            "SUMMARY:Plan trip to Oslo\r\nLAST-MODIFIED:20261021T130000Z\r\n",
        ),
    );
    g.set_now(Some(ms(2026, 10, 21, 14)));
    g.update_block(b.id, "TODO Plan trip to Lima").unwrap();
    g.set_now(Some(ms(2026, 10, 21, 15)));
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.conflicts, 1);
    assert_eq!(
        g.get_block(b.id).unwrap().unwrap().content,
        "TODO Plan trip to Lima"
    );
    assert!(m.body(&path).unwrap().contains("SUMMARY:Plan trip to Lima"));
    let tree = page_blocks(&g, "Tasks");
    assert!(
        tree.iter()
            .any(|(d, x)| *d == 1 && x.content.contains("remote") && x.content.contains("Oslo"))
    );
}

#[tokio::test]
async fn stale_etag_on_push_refetches_and_retries() {
    let m = Mock::start().await;
    let path = format!("{TASKS}r.ics");
    m.set(
        &path,
        &vtodo("r", "SUMMARY:Race\r\nLAST-MODIFIED:20261021T080000Z\r\n"),
    );
    let (_d, g) = graph();
    let cfg = Config::new(Madrid);
    let c = client(&m);
    sync(&g, &c, &cfg).await.unwrap();
    let b = page_blocks(&g, "Tasks")[0].1.clone();
    g.set_now(Some(ms(2026, 10, 21, 10)));
    g.update_block(b.id, "TODO Race local").unwrap();
    // Our cached etag is out of date although the pull sees no change since its token.
    g.conn()
        .execute("UPDATE caldav_object SET etag='\"stale\"'", [])
        .unwrap();
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.conflicts, 1);
    assert_eq!(rep.tasks_updated_remote, 1);
    assert!(m.body(&path).unwrap().contains("SUMMARY:Race local"));
    let puts = m.state.lock().unwrap().puts.clone();
    assert_eq!(
        puts.len(),
        2,
        "first PUT rejected with 412, second accepted: {puts:?}"
    );
}

#[tokio::test]
async fn deletions_both_ways_and_ctag_fallback() {
    let m = Mock::start().await;
    m.disable_sync_collection();
    m.set(&format!("{TASKS}a.ics"), &vtodo("a", "SUMMARY:Alpha\r\n"));
    m.set(&format!("{TASKS}b.ics"), &vtodo("b", "SUMMARY:Beta\r\n"));
    let (_d, g) = graph();
    let cfg = Config::new(Madrid);
    let c = client(&m);
    sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(page_blocks(&g, "Tasks").len(), 2);
    assert_eq!(
        m.count_requests("REPORT"),
        3,
        "two refused sync-collections + one multiget"
    );

    // Unchanged ctag: the second sync lists nothing.
    let before = m.count_requests("depth");
    sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(m.count_requests("depth"), before);

    // Remote delete of Alpha -> block gone; local delete of Beta -> remote gone.
    m.remove(&format!("{TASKS}a.ics"));
    let beta = page_blocks(&g, "Tasks")
        .into_iter()
        .find(|(_, b)| b.content.contains("Beta"))
        .unwrap()
        .1;
    g.delete_block(beta.id).unwrap();
    let rep = sync(&g, &c, &cfg).await.unwrap();
    assert_eq!(rep.removed_remote, 1);
    assert_eq!(rep.tasks_deleted_remote, 1);
    assert!(page_blocks(&g, "Tasks").is_empty());
    assert!(m.paths(TASKS).is_empty());
}
