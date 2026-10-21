//! `caldav_*` tables living in the graph database (sync tokens, etags, uid <-> block ids).

use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS caldav_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS caldav_calendar (
    href TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    events INTEGER NOT NULL,
    tasks INTEGER NOT NULL,
    ctag TEXT,
    sync_token TEXT
);
-- One row per remote calendar object. `ics` is the last server version (needed to edit a task
-- without losing unknown properties and to answer events_for_day offline).
CREATE TABLE IF NOT EXISTS caldav_object (
    href TEXT PRIMARY KEY,
    calendar TEXT NOT NULL,
    uid TEXT NOT NULL,
    kind TEXT NOT NULL,
    etag TEXT NOT NULL,
    ics TEXT NOT NULL,
    block_id TEXT,
    synced_content TEXT
);
CREATE INDEX IF NOT EXISTS caldav_object_cal ON caldav_object(calendar, kind);
CREATE INDEX IF NOT EXISTS caldav_object_uid ON caldav_object(uid);
CREATE INDEX IF NOT EXISTS caldav_object_block ON caldav_object(block_id);
-- Event occurrences materialised as journal blocks.
CREATE TABLE IF NOT EXISTS caldav_event_block (
    uid TEXT NOT NULL,
    day TEXT NOT NULL,
    block_id TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY (uid, day, block_id)
);
"#;

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CalendarRow {
    pub href: String,
    pub name: String,
    pub events: bool,
    pub tasks: bool,
    pub ctag: Option<String>,
    pub sync_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ObjectRow {
    pub href: String,
    pub calendar: String,
    pub uid: String,
    pub kind: String,
    pub etag: String,
    pub ics: String,
    pub block_id: Option<String>,
    pub synced_content: Option<String>,
}

const OBJ_COLS: &str = "href, calendar, uid, kind, etag, ics, block_id, synced_content";

fn obj_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectRow> {
    Ok(ObjectRow {
        href: r.get(0)?,
        calendar: r.get(1)?,
        uid: r.get(2)?,
        kind: r.get(3)?,
        etag: r.get(4)?,
        ics: r.get(5)?,
        block_id: r.get(6)?,
        synced_content: r.get(7)?,
    })
}

pub fn meta_get(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(
        c.query_row("SELECT value FROM caldav_meta WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()?,
    )
}

pub fn meta_set(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute(
        "INSERT INTO caldav_meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2",
        [key, value],
    )?;
    Ok(())
}

pub fn calendars(c: &Connection) -> Result<Vec<CalendarRow>> {
    let mut st = c.prepare("SELECT href, name, events, tasks, ctag, sync_token FROM caldav_calendar ORDER BY name, href")?;
    let rows = st
        .query_map([], |r| {
            Ok(CalendarRow {
                href: r.get(0)?,
                name: r.get(1)?,
                events: r.get::<_, i64>(2)? != 0,
                tasks: r.get::<_, i64>(3)? != 0,
                ctag: r.get(4)?,
                sync_token: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

pub fn calendar(c: &Connection, href: &str) -> Result<Option<CalendarRow>> {
    Ok(calendars(c)?.into_iter().find(|r| r.href == href))
}

/// Inserts or updates a calendar's descriptive fields, keeping its sync state.
pub fn upsert_calendar(
    c: &Connection,
    name: &str,
    href: &str,
    events: bool,
    tasks: bool,
) -> Result<()> {
    c.execute(
        "INSERT INTO caldav_calendar(href, name, events, tasks) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(href) DO UPDATE SET name=?2, events=?3, tasks=?4",
        params![href, name, events, tasks],
    )?;
    Ok(())
}

pub fn set_calendar_state(
    c: &Connection,
    href: &str,
    ctag: Option<&str>,
    token: Option<&str>,
) -> Result<()> {
    c.execute(
        "UPDATE caldav_calendar SET ctag=COALESCE(?2, ctag), sync_token=COALESCE(?3, sync_token) WHERE href=?1",
        params![href, ctag, token],
    )?;
    Ok(())
}

pub fn clear_calendar_token(c: &Connection, href: &str) -> Result<()> {
    c.execute(
        "UPDATE caldav_calendar SET sync_token=NULL, ctag=NULL WHERE href=?1",
        [href],
    )?;
    Ok(())
}

pub fn object(c: &Connection, href: &str) -> Result<Option<ObjectRow>> {
    Ok(c.query_row(
        &format!("SELECT {OBJ_COLS} FROM caldav_object WHERE href=?1"),
        [href],
        obj_row,
    )
    .optional()?)
}

pub fn object_by_block(c: &Connection, block: &str) -> Result<Option<ObjectRow>> {
    Ok(c.query_row(
        &format!("SELECT {OBJ_COLS} FROM caldav_object WHERE block_id=?1"),
        [block],
        obj_row,
    )
    .optional()?)
}

pub fn objects(
    c: &Connection,
    calendar: Option<&str>,
    kind: Option<&str>,
) -> Result<Vec<ObjectRow>> {
    let mut st = c.prepare(&format!(
        "SELECT {OBJ_COLS} FROM caldav_object WHERE (?1 IS NULL OR calendar=?1) AND (?2 IS NULL OR kind=?2) ORDER BY href"
    ))?;
    let rows = st
        .query_map(params![calendar, kind], obj_row)?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

pub fn put_object(c: &Connection, o: &ObjectRow) -> Result<()> {
    c.execute(
        &format!(
            "INSERT INTO caldav_object({OBJ_COLS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(href) DO UPDATE SET calendar=?2, uid=?3, kind=?4, etag=?5, ics=?6, block_id=?7, synced_content=?8"
        ),
        params![o.href, o.calendar, o.uid, o.kind, o.etag, o.ics, o.block_id, o.synced_content],
    )?;
    Ok(())
}

pub fn delete_object(c: &Connection, href: &str) -> Result<()> {
    c.execute("DELETE FROM caldav_object WHERE href=?1", [href])?;
    Ok(())
}
