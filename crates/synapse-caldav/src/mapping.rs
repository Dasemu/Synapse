//! Synapse task syntax <-> VTODO fields.
//!
//! ```text
//! TODO [#A] Pay rent
//! SCHEDULED: <2026-10-21 Wed>
//! DEADLINE: <2026-10-25 Sun 14:00>
//! free-form notes (become DESCRIPTION)
//! ```

use crate::calendar::{DtValue, Todo, TodoStatus};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Todo,
    Later,
    Doing,
    Now,
    Done,
    Canceled,
}

impl Marker {
    pub const ALL: [(&'static str, Marker); 7] = [
        ("TODO", Marker::Todo),
        ("LATER", Marker::Later),
        ("DOING", Marker::Doing),
        ("NOW", Marker::Now),
        ("DONE", Marker::Done),
        ("CANCELED", Marker::Canceled),
        ("CANCELLED", Marker::Canceled),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Marker::Todo => "TODO",
            Marker::Later => "LATER",
            Marker::Doing => "DOING",
            Marker::Now => "NOW",
            Marker::Done => "DONE",
            Marker::Canceled => "CANCELED",
        }
    }

    pub fn status(self) -> TodoStatus {
        match self {
            Marker::Todo | Marker::Later => TodoStatus::NeedsAction,
            Marker::Doing | Marker::Now => TodoStatus::InProcess,
            Marker::Done => TodoStatus::Completed,
            Marker::Canceled => TodoStatus::Cancelled,
        }
    }

    /// Marker for a remote status, keeping `prev` when it is an equivalent spelling.
    pub fn for_status(s: TodoStatus, prev: Option<Marker>) -> Marker {
        match prev {
            Some(p) if p.status() == s => p,
            _ => match s {
                TodoStatus::NeedsAction => Marker::Todo,
                TodoStatus::InProcess => Marker::Doing,
                TodoStatus::Completed => Marker::Done,
                TodoStatus::Cancelled => Marker::Canceled,
            },
        }
    }
}

/// Parsed form of a task block's content.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskText {
    pub marker: Marker,
    pub priority: Option<char>,
    pub title: String,
    pub scheduled: Option<DtValue>,
    pub deadline: Option<DtValue>,
    pub notes: String,
}

fn split_marker(content: &str) -> Option<(Marker, &str)> {
    let first = content.lines().next()?;
    Marker::ALL.iter().find_map(|(word, m)| {
        let rest = first.strip_prefix(word)?;
        (rest.is_empty() || rest.starts_with(' ')).then_some((*m, rest))
    })
}

/// Detects the marker at the start of `content` (first line).
pub fn marker_of(content: &str) -> Option<Marker> {
    split_marker(content).map(|(m, _)| m)
}

fn parse_ldate(s: &str) -> Option<DtValue> {
    // `<2026-10-21 Wed>` or `<2026-10-21 Wed 14:00>`; repeaters (`+1w`) are ignored.
    let inner = s.trim().strip_prefix('<')?.split('>').next()?;
    let mut parts = inner.split_whitespace();
    let date = NaiveDate::parse_from_str(parts.next()?, "%Y-%m-%d").ok()?;
    let time = parts.find_map(|p| NaiveTime::parse_from_str(p, "%H:%M").ok());
    Some(match time {
        Some(t) => DtValue::Floating(NaiveDateTime::new(date, t)),
        None => DtValue::Date(date),
    })
}

fn fmt_ldate(v: &DtValue, view: Tz) -> String {
    let n = v.local(view);
    let day = n.format("%Y-%m-%d %a");
    if v.is_date() {
        format!("<{day}>")
    } else {
        format!("<{day} {}>", n.format("%H:%M"))
    }
}

pub fn parse_task(content: &str) -> Option<TaskText> {
    let (marker, rest) = split_marker(content)?;
    let mut lines = content.lines();
    lines.next();
    let mut title = rest.trim().to_string();
    let mut priority = None;
    if let Some(rest) = title.strip_prefix("[#")
        && let Some((p, tail)) = rest.split_once(']')
        && p.len() == 1
    {
        priority = p.chars().next().map(|c| c.to_ascii_uppercase());
        title = tail.trim().to_string();
    }
    let (mut scheduled, mut deadline) = (None, None);
    let mut notes = Vec::new();
    for l in lines {
        let t = l.trim();
        if let Some(r) = t.strip_prefix("SCHEDULED:") {
            scheduled = parse_ldate(r);
        } else if let Some(r) = t.strip_prefix("DEADLINE:") {
            deadline = parse_ldate(r);
        } else {
            notes.push(l);
        }
    }
    Some(TaskText {
        marker,
        priority,
        title,
        scheduled,
        deadline,
        notes: notes.join("\n").trim().to_string(),
    })
}

pub fn render_task(t: &TaskText, view: Tz) -> String {
    let mut out = t.marker.as_str().to_string();
    if let Some(p) = t.priority {
        out.push_str(&format!(" [#{p}]"));
    }
    if !t.title.is_empty() {
        out.push(' ');
        out.push_str(&t.title);
    }
    if let Some(s) = &t.scheduled {
        out.push_str(&format!("\nSCHEDULED: {}", fmt_ldate(s, view)));
    }
    if let Some(d) = &t.deadline {
        out.push_str(&format!("\nDEADLINE: {}", fmt_ldate(d, view)));
    }
    if !t.notes.is_empty() {
        out.push('\n');
        out.push_str(&t.notes);
    }
    out
}

pub fn priority_to_ical(p: Option<char>) -> u8 {
    match p {
        Some('A') => 1,
        Some('B') => 5,
        Some('C') => 9,
        _ => 0,
    }
}

pub fn priority_from_ical(p: u8) -> Option<char> {
    match p {
        1..=4 => Some('A'),
        5 => Some('B'),
        6..=9 => Some('C'),
        _ => None,
    }
}

/// Re-zones a floating local time so that the remote keeps an unambiguous instant.
fn to_remote(v: DtValue, view: Tz) -> DtValue {
    match v {
        DtValue::Floating(n) => DtValue::Zoned(view, n),
        o => o,
    }
}

/// Task fields from block content. `existing` supplies the fields Synapse does not model
/// (percent, COMPLETED timestamp) and the previous values for unchanged dates.
pub fn todo_from_text(
    t: &TaskText,
    uid: &str,
    existing: Option<&Todo>,
    view: Tz,
    now: DateTime<Utc>,
) -> Todo {
    let status = t.marker.status();
    // Keep the original zoned value when the local rendering did not change.
    let keep = |new: &Option<DtValue>, old: Option<&DtValue>| match (new, old) {
        (Some(n), Some(o)) if n.local(view) == o.local(view) && n.is_date() == o.is_date() => {
            Some(o.clone())
        }
        (Some(n), _) => Some(to_remote(n.clone(), view)),
        (None, _) => None,
    };
    Todo {
        uid: uid.to_string(),
        summary: t.title.clone(),
        description: (!t.notes.is_empty()).then(|| t.notes.clone()),
        status,
        percent: existing.and_then(|e| e.percent),
        priority: priority_to_ical(t.priority),
        start: keep(&t.scheduled, existing.and_then(|e| e.start.as_ref())),
        due: keep(&t.deadline, existing.and_then(|e| e.due.as_ref())),
        completed: match status {
            TodoStatus::Completed => Some(existing.and_then(|e| e.completed).unwrap_or(now)),
            _ => None,
        },
        last_modified: Some(now),
        has_rrule: existing.is_some_and(|e| e.has_rrule),
    }
}

pub fn text_from_todo(t: &Todo, prev: Option<Marker>) -> TaskText {
    TaskText {
        marker: Marker::for_status(t.status, prev),
        priority: priority_from_ical(t.priority),
        title: t.summary.clone(),
        scheduled: t.start.clone(),
        deadline: t.due.clone(),
        notes: t.description.clone().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_render() {
        let c = "TODO [#A] Pay rent\nSCHEDULED: <2026-10-21 Wed>\nDEADLINE: <2026-10-25 Sun 14:00>\nsome note";
        let t = parse_task(c).unwrap();
        assert_eq!(t.marker, Marker::Todo);
        assert_eq!(t.priority, Some('A'));
        assert_eq!(t.title, "Pay rent");
        assert_eq!(t.notes, "some note");
        assert!(t.scheduled.as_ref().unwrap().is_date());
        assert_eq!(render_task(&t, chrono_tz::UTC), c);
        assert!(parse_task("TODOS x").is_none());
        assert_eq!(parse_task("DONE").unwrap().title, "");
        assert_eq!(priority_from_ical(priority_to_ical(Some('B'))), Some('B'));
    }
}
