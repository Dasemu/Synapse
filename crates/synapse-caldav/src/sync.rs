//! Two-way synchronisation between a CalDAV account and a Synapse graph.
//!
//! The futures hold `&Graph` (which is `Send` but not `Sync`) across awaits, so they are not
//! `Send`: drive them with `block_on` on a current-thread runtime / dedicated thread.

use crate::calendar::{
    self, Event, Todo, event_occurrences, master_todo, master_todo_mut, parse_calendar,
};
use crate::dav::{CalendarInfo, DavClient, RemoteObject};
use crate::mapping::{self, Marker, TaskText};
use crate::store::{self, ObjectRow};
use crate::{Error, Result, ical};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use synapse_core::{Block, BlockId, Graph, Placement, Properties};
use uuid::Uuid;

pub const PROP_UID: &str = "caldav-uid";
pub const PROP_CALENDAR: &str = "caldav-calendar";
pub const PROP_EVENT: &str = "caldav-event";

#[derive(Debug, Clone)]
pub struct Config {
    /// Zone used to show UTC/zoned times and to interpret floating ones.
    pub tz: Tz,
    /// Href of the task list that receives new tasks written in journals
    /// (default: the first calendar that supports VTODO).
    pub default_task_calendar: Option<String>,
}

impl Config {
    pub fn new(tz: Tz) -> Self {
        Config {
            tz,
            default_task_calendar: None,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub calendars: usize,
    pub events_changed: usize,
    pub tasks_pulled: usize,
    pub removed_remote: usize,
    pub tasks_created_remote: usize,
    pub tasks_updated_remote: usize,
    pub tasks_deleted_remote: usize,
    pub conflicts: usize,
}

fn now_utc(graph: &Graph) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(graph.now_ms())
        .single()
        .unwrap_or_else(Utc::now)
}

/// Discovers the account's calendars and records them locally (sync state is preserved).
pub async fn refresh_calendars(graph: &Graph, client: &DavClient) -> Result<Vec<CalendarInfo>> {
    store::ensure_schema(graph.conn())?;
    let home = match store::meta_get(graph.conn(), "home")? {
        Some(h) => h,
        None => {
            let h = client.discover().await?;
            store::meta_set(graph.conn(), "home", &h)?;
            h
        }
    };
    let cals = client.list_calendars(&home).await?;
    for c in &cals {
        if c.supports_events || c.supports_tasks {
            store::upsert_calendar(
                graph.conn(),
                &c.name,
                &c.href,
                c.supports_events,
                c.supports_tasks,
            )?;
        }
    }
    Ok(cals)
}

/// Full two-way sync: pull every calendar, then push local task changes.
pub async fn sync(graph: &Graph, client: &DavClient, cfg: &Config) -> Result<SyncReport> {
    store::ensure_schema(graph.conn())?;
    if store::meta_get(graph.conn(), "enabled_at")?.is_none() {
        store::meta_set(graph.conn(), "enabled_at", &graph.now_ms().to_string())?;
    }
    refresh_calendars(graph, client).await?;
    let mut rep = SyncReport::default();
    let cals = store::calendars(graph.conn())?;
    rep.calendars = cals.len();
    for cal in &cals {
        pull_calendar(graph, client, cfg, cal, &mut rep).await?;
    }
    push_tasks(graph, client, cfg, &mut rep).await?;
    Ok(rep)
}

// ------------------------------------------------------------------------------- pull

async fn pull_calendar(
    graph: &Graph,
    client: &DavClient,
    cfg: &Config,
    cal: &store::CalendarRow,
    rep: &mut SyncReport,
) -> Result<()> {
    let known: std::collections::HashMap<String, String> =
        store::objects(graph.conn(), Some(&cal.href), None)?
            .into_iter()
            .map(|o| (o.href, o.etag))
            .collect();
    type Pulled = (
        Vec<(String, String)>,
        Vec<String>,
        Option<String>,
        Option<String>,
    );
    let (changed, deleted, new_token, new_ctag): Pulled;
    let incremental = match client
        .sync_collection(&cal.href, cal.sync_token.as_deref())
        .await?
    {
        Some(ch) => Some(ch),
        None if cal.sync_token.is_some() => {
            // Token rejected: restart with a full sync-collection.
            store::clear_calendar_token(graph.conn(), &cal.href)?;
            client.sync_collection(&cal.href, None).await?
        }
        None => None,
    };
    if let Some(ch) = incremental {
        changed = ch.changed;
        deleted = ch.deleted;
        new_token = ch.new_token;
        new_ctag = None;
    } else {
        // ctag/etag fallback.
        let ctag = client.ctag(&cal.href).await?;
        if ctag.is_some() && ctag == cal.ctag {
            return Ok(());
        }
        let all = client.list_etags(&cal.href).await?;
        let present: std::collections::HashSet<&String> = all.iter().map(|(h, _)| h).collect();
        deleted = known
            .keys()
            .filter(|h| !present.contains(h))
            .cloned()
            .collect();
        changed = all.to_vec();
        new_token = None;
        new_ctag = ctag;
    }
    for h in &deleted {
        remove_remote(graph, h, rep)?;
    }
    let want: Vec<String> = changed
        .iter()
        .filter(|(h, e)| known.get(h) != Some(e))
        .map(|(h, _)| h.clone())
        .collect();
    if !want.is_empty() {
        for obj in client.multiget(&cal.href, &want).await? {
            apply_remote(graph, cfg, &cal.href, &cal.name, &obj, rep)?;
        }
    }
    store::set_calendar_state(
        graph.conn(),
        &cal.href,
        new_ctag.as_deref(),
        new_token.as_deref(),
    )?;
    Ok(())
}

fn merge_props(block: &Block, uid: &str, cal: &str) -> Properties {
    let mut p = block.properties.clone();
    p.insert(PROP_UID.into(), Value::String(uid.into()));
    p.insert(PROP_CALENDAR.into(), Value::String(cal.into()));
    p
}

fn conflict_note(graph: &Graph, block: &Block, label: &str, content: &str) -> Result<()> {
    let note = format!("CalDAV conflict: {label} version (older, kept as note)\n{content}");
    graph.insert_block(block.page_id, Placement::LastChildOf(Some(block.id)), &note)?;
    Ok(())
}

fn remove_remote(graph: &Graph, href: &str, rep: &mut SyncReport) -> Result<()> {
    let Some(row) = store::object(graph.conn(), href)? else {
        return Ok(());
    };
    rep.removed_remote += 1;
    if let Some(bid) = row
        .block_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        && let Some(block) = graph.get_block(bid)?
    {
        if Some(&block.content) == row.synced_content.as_ref() {
            graph.delete_block(bid)?;
        } else {
            // Local edits win over a remote delete: detach and let the push re-create it.
            let mut p = block.properties.clone();
            p.remove(PROP_UID);
            p.remove(PROP_CALENDAR);
            graph.set_block_properties(bid, p)?;
        }
    }
    store::delete_object(graph.conn(), href)
}

fn apply_remote(
    graph: &Graph,
    cfg: &Config,
    cal_href: &str,
    cal_name: &str,
    obj: &RemoteObject,
    rep: &mut SyncReport,
) -> Result<()> {
    let cal = parse_calendar(&obj.ics)?;
    let is_task = cal.children_named("VTODO").next().is_some();
    let is_event = cal.children_named("VEVENT").next().is_some();
    let comp = if is_task {
        master_todo(&cal)
    } else {
        cal.children_named("VEVENT").next()
    };
    let Some(comp) = comp else { return Ok(()) };
    let Some(uid) = comp.text("UID") else {
        return Ok(());
    };
    let existing = store::object(graph.conn(), &obj.href)?;

    if !is_task {
        if is_event {
            rep.events_changed += 1;
        }
        store::put_object(
            graph.conn(),
            &ObjectRow {
                href: obj.href.clone(),
                calendar: cal_href.to_string(),
                uid,
                kind: if is_event { "VEVENT" } else { "OTHER" }.into(),
                etag: obj.etag.clone(),
                ics: obj.ics.clone(),
                block_id: None,
                synced_content: None,
            },
        )?;
        return Ok(());
    }

    let Some(todo) = Todo::from_component(comp) else {
        return Ok(());
    };
    rep.tasks_pulled += 1;
    let block = existing
        .as_ref()
        .and_then(|r| r.block_id.as_deref())
        .and_then(|s| Uuid::parse_str(s).ok())
        .map(|id| graph.get_block(id))
        .transpose()?
        .flatten();
    let mut row = ObjectRow {
        href: obj.href.clone(),
        calendar: cal_href.to_string(),
        uid: uid.clone(),
        kind: "VTODO".into(),
        etag: obj.etag.clone(),
        ics: obj.ics.clone(),
        block_id: existing.as_ref().and_then(|r| r.block_id.clone()),
        synced_content: existing.as_ref().and_then(|r| r.synced_content.clone()),
    };
    match block {
        None if existing.as_ref().is_some_and(|r| r.block_id.is_some()) => {
            // Deleted locally: keep the old etag so the push phase can delete remotely.
            return Ok(());
        }
        None => {
            let page = graph.ensure_page(cal_name, None)?;
            let content = mapping::render_task(&mapping::text_from_todo(&todo, None), cfg.tz);
            let b = graph.insert_block(page.id, Placement::LastChildOf(None), &content)?;
            graph.set_block_properties(b.id, merge_props(&b, &uid, cal_href))?;
            row.block_id = Some(b.id.to_string());
            row.synced_content = Some(content);
        }
        Some(b) => {
            let prev = mapping::marker_of(&b.content);
            let content = mapping::render_task(&mapping::text_from_todo(&todo, prev), cfg.tz);
            let local_changed = Some(&b.content) != row.synced_content.as_ref();
            if !local_changed || b.content == content {
                if b.content != content {
                    graph.update_block(b.id, &content)?;
                }
                row.synced_content = Some(content);
            } else {
                rep.conflicts += 1;
                let remote_ms = todo
                    .last_modified
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0);
                if remote_ms >= b.updated_at {
                    conflict_note(graph, &b, "local", &b.content)?;
                    graph.update_block(b.id, &content)?;
                    row.synced_content = Some(content);
                } else {
                    // Local is newer: keep it (the push phase uploads it) and note the remote.
                    conflict_note(graph, &b, "remote", &content)?;
                }
            }
        }
    }
    store::put_object(graph.conn(), &row)
}

// ------------------------------------------------------------------------------- push

fn default_task_calendar(graph: &Graph, cfg: &Config) -> Result<Option<store::CalendarRow>> {
    let cals = store::calendars(graph.conn())?;
    Ok(match &cfg.default_task_calendar {
        Some(h) => cals.into_iter().find(|c| &c.href == h),
        None => cals.into_iter().find(|c| c.tasks),
    })
}

async fn push_tasks(
    graph: &Graph,
    client: &DavClient,
    cfg: &Config,
    rep: &mut SyncReport,
) -> Result<()> {
    // Tracked tasks: edits and deletions.
    for row in store::objects(graph.conn(), None, Some("VTODO"))? {
        if row.block_id.is_none() {
            continue;
        }
        push_tracked(graph, client, cfg, row, rep).await?;
    }
    // New tasks typed into journals.
    let Some(cal) = default_task_calendar(graph, cfg)? else {
        return Ok(());
    };
    let since: i64 = store::meta_get(graph.conn(), "enabled_at")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut st = graph.conn().prepare(
        "SELECT b.id FROM blocks b JOIN pages p ON p.id=b.page_id
         WHERE p.journal_date IS NOT NULL AND b.updated_at >= ?1
           AND b.properties NOT LIKE '%caldav-uid%' ORDER BY b.created_at",
    )?;
    let ids: Vec<String> = st
        .query_map([since], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(st);
    for id in ids {
        let Some(b) = Uuid::parse_str(&id)
            .ok()
            .map(|i| graph.get_block(i))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        let Some(task) = mapping::parse_task(&b.content) else {
            continue;
        };
        if matches!(task.marker, Marker::Done | Marker::Canceled) {
            continue;
        }
        let uid = Uuid::now_v7().to_string();
        let todo = mapping::todo_from_text(&task, &uid, None, cfg.tz, now_utc(graph));
        let ics = ical::serialize(&todo.to_calendar(now_utc(graph)));
        let href = format!("{}/{}.ics", cal.href.trim_end_matches('/'), uid);
        let etag = client.put(&href, &ics, None).await?;
        graph.set_block_properties(b.id, merge_props(&b, &uid, &cal.href))?;
        store::put_object(
            graph.conn(),
            &ObjectRow {
                href,
                calendar: cal.href.clone(),
                uid,
                kind: "VTODO".into(),
                etag,
                ics,
                block_id: Some(b.id.to_string()),
                synced_content: Some(b.content.clone()),
            },
        )?;
        rep.tasks_created_remote += 1;
    }
    Ok(())
}

async fn push_tracked(
    graph: &Graph,
    client: &DavClient,
    cfg: &Config,
    row: ObjectRow,
    rep: &mut SyncReport,
) -> Result<()> {
    let mut row = row;
    for attempt in 0..2 {
        let block = row
            .block_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
            .map(|id| graph.get_block(id))
            .transpose()?
            .flatten();
        let Some(block) = block else {
            // Deleted locally.
            match client.delete(&row.href, Some(&row.etag)).await {
                Ok(()) => {
                    store::delete_object(graph.conn(), &row.href)?;
                    rep.tasks_deleted_remote += 1;
                }
                Err(Error::PreconditionFailed(_)) => {
                    // Changed remotely meanwhile: the remote edit wins, bring it back.
                    let obj = client.get(&row.href).await?;
                    store::delete_object(graph.conn(), &row.href)?;
                    let name = store::calendar(graph.conn(), &row.calendar)?
                        .map(|c| c.name)
                        .unwrap_or_default();
                    apply_remote(graph, cfg, &row.calendar, &name, &obj, rep)?;
                }
                Err(e) => return Err(e),
            }
            return Ok(());
        };
        if Some(&block.content) == row.synced_content.as_ref() {
            return Ok(());
        }
        let Some(task) = mapping::parse_task(&block.content) else {
            return Ok(()); // no longer a task; leave the remote alone
        };
        let mut cal = parse_calendar(&row.ics)?;
        let existing = master_todo(&cal).and_then(Todo::from_component);
        let now = now_utc(graph);
        let todo = mapping::todo_from_text(&task, &row.uid, existing.as_ref(), cfg.tz, now);
        let Some(comp) = master_todo_mut(&mut cal) else {
            return Ok(());
        };
        todo.apply_to(comp, now);
        let ics = ical::serialize(&cal);
        match client.put(&row.href, &ics, Some(&row.etag)).await {
            Ok(etag) => {
                row.etag = etag;
                row.ics = ics;
                row.synced_content = Some(block.content.clone());
                store::put_object(graph.conn(), &row)?;
                rep.tasks_updated_remote += 1;
                return Ok(());
            }
            Err(Error::PreconditionFailed(_)) if attempt == 0 => {
                // Both sides changed: resolve by last-modified, then retry once.
                let obj = client.get(&row.href).await?;
                let name = store::calendar(graph.conn(), &row.calendar)?
                    .map(|c| c.name)
                    .unwrap_or_default();
                apply_remote(graph, cfg, &row.calendar, &name, &obj, rep)?;
                row = store::object(graph.conn(), &row.href)?
                    .ok_or_else(|| Error::Other("object vanished".into()))?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------ events

/// Events (with recurrences expanded) overlapping `day`, from the local cache.
pub fn events_for_day(graph: &Graph, day: NaiveDate, tz: Tz) -> Result<Vec<calendar::Occurrence>> {
    events_between(graph, day, day + Duration::days(1), tz)
}

/// Events overlapping `[from, to)` (dates), from the local cache.
pub fn events_between(
    graph: &Graph,
    from: NaiveDate,
    to: NaiveDate,
    tz: Tz,
) -> Result<Vec<calendar::Occurrence>> {
    store::ensure_schema(graph.conn())?;
    let names: std::collections::HashMap<String, String> = store::calendars(graph.conn())?
        .into_iter()
        .map(|c| (c.href, c.name))
        .collect();
    let (f, t) = (from.and_time(NaiveTime::MIN), to.and_time(NaiveTime::MIN));
    let mut out = Vec::new();
    for o in store::objects(graph.conn(), None, Some("VEVENT"))? {
        let Ok(cal) = parse_calendar(&o.ics) else {
            continue;
        };
        let name = names.get(&o.calendar).cloned().unwrap_or_default();
        out.extend(event_occurrences(&cal, &name, &o.href, tz, f, t));
    }
    out.sort_by_key(|o| (o.start, o.summary.clone()));
    Ok(out)
}

fn clean(s: &str) -> String {
    s.replace("[[", "[ [")
        .replace("]]", "] ]")
        .replace('\n', " ")
}

fn occurrence_text(o: &calendar::Occurrence) -> String {
    let when = if o.all_day {
        "All day".to_string()
    } else {
        format!("{}-{}", o.start.format("%H:%M"), o.end.format("%H:%M"))
    };
    let title = clean(&o.summary);
    // A page per recurring meeting collects its notes across occurrences.
    let title = if o.recurring && !title.is_empty() {
        format!("[[{title}]]")
    } else {
        title
    };
    format!("{when} {title}")
}

/// Writes the day's events into its journal as blocks (idempotent; updates changed events and
/// removes blocks of events that vanished). Returns the block ids, in time order.
pub fn materialize_day(graph: &Graph, day: NaiveDate, tz: Tz) -> Result<Vec<BlockId>> {
    let occs = events_for_day(graph, day, tz)?;
    let journal = graph.journal(day)?;
    let day_s = day.to_string();
    let mut ids = Vec::new();
    let mut keep = Vec::new();
    for o in &occs {
        let text = occurrence_text(o);
        let key = format!("{}@{}", o.uid, o.start.format("%Y%m%dT%H%M%S"));
        let existing: Option<(String, String)> = graph
            .conn()
            .query_row(
                "SELECT block_id, content FROM caldav_event_block WHERE uid=?1 AND day=?2",
                [&key, &day_s],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let block = match existing {
            Some((bid, old)) => match Uuid::parse_str(&bid)
                .ok()
                .map(|i| graph.get_block(i))
                .transpose()?
                .flatten()
            {
                Some(b) => {
                    // Do not overwrite text the user edited by hand.
                    if b.content == old && old != text {
                        graph.update_block(b.id, &text)?;
                    }
                    b
                }
                None => new_event_block(graph, &journal, &text, &o.uid)?,
            },
            None => new_event_block(graph, &journal, &text, &o.uid)?,
        };
        graph.conn().execute(
            "DELETE FROM caldav_event_block WHERE uid=?1 AND day=?2",
            [&key, &day_s],
        )?;
        graph.conn().execute(
            "INSERT INTO caldav_event_block(uid, day, block_id, content) VALUES(?1,?2,?3,?4)",
            [&key, &day_s, &block.id.to_string(), &text],
        )?;
        keep.push(key);
        ids.push(block.id);
    }
    // Events no longer on that day.
    let mut st = graph
        .conn()
        .prepare("SELECT uid, block_id, content FROM caldav_event_block WHERE day=?1")?;
    let rows: Vec<(String, String, String)> = st
        .query_map([&day_s], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(st);
    for (key, bid, content) in rows {
        if keep.contains(&key) {
            continue;
        }
        if let Some(b) = Uuid::parse_str(&bid)
            .ok()
            .map(|i| graph.get_block(i))
            .transpose()?
            .flatten()
            && b.content == content
        {
            graph.delete_block(b.id)?;
        }
        graph.conn().execute(
            "DELETE FROM caldav_event_block WHERE uid=?1 AND day=?2",
            [&key, &day_s],
        )?;
    }
    Ok(ids)
}

fn new_event_block(
    graph: &Graph,
    journal: &synapse_core::Page,
    text: &str,
    uid: &str,
) -> Result<Block> {
    let b = graph.insert_block(journal.id, Placement::LastChildOf(None), text)?;
    let mut p = b.properties.clone();
    p.insert(PROP_EVENT.into(), Value::String(uid.into()));
    Ok(graph.set_block_properties(b.id, p)?)
}

/// Helper for UIs: the event model behind a cached object.
pub fn event_of(ics: &str) -> Option<Event> {
    let cal = parse_calendar(ics).ok()?;
    cal.children_named("VEVENT").find_map(Event::from_component)
}

/// Marks a task block done/undone locally (the next sync pushes it).
pub fn set_task_done(graph: &Graph, id: BlockId, done: bool) -> Result<()> {
    let b = graph
        .get_block(id)?
        .ok_or_else(|| Error::Other("block not found".into()))?;
    let Some(t) = mapping::parse_task(&b.content) else {
        return Err(Error::Other("not a task block".into()));
    };
    let new = TaskText {
        marker: if done { Marker::Done } else { Marker::Todo },
        ..t
    };
    graph.update_block(id, &mapping::render_task(&new, chrono_tz::UTC))?;
    Ok(())
}
