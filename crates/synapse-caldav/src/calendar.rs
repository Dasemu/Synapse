//! Typed views over iCalendar components: date-times, events, tasks and RRULE expansion.

use crate::ical::{Component, Prop, escape};
use crate::{Error, Result, ical};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

/// An iCalendar DATE / DATE-TIME value with its zone semantics preserved.
#[derive(Debug, Clone, PartialEq)]
pub enum DtValue {
    Date(NaiveDate),
    /// No zone: wall-clock time in the viewer's zone.
    Floating(NaiveDateTime),
    Utc(DateTime<Utc>),
    Zoned(Tz, NaiveDateTime),
}

impl DtValue {
    pub fn parse(p: &Prop) -> Option<DtValue> {
        let v = p.value.trim();
        if p.param("VALUE") == Some("DATE") || (v.len() == 8 && !v.contains('T')) {
            return NaiveDate::parse_from_str(v, "%Y%m%d")
                .ok()
                .map(DtValue::Date);
        }
        if let Some(z) = v.strip_suffix('Z') {
            let n = NaiveDateTime::parse_from_str(z, "%Y%m%dT%H%M%S").ok()?;
            return Some(DtValue::Utc(Utc.from_utc_datetime(&n)));
        }
        let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
        match p.param("TZID").and_then(|t| t.parse::<Tz>().ok()) {
            Some(tz) => Some(DtValue::Zoned(tz, n)),
            None => Some(DtValue::Floating(n)),
        }
    }

    pub fn to_prop(&self, name: &str) -> Prop {
        match self {
            DtValue::Date(d) => {
                Prop::new(name, d.format("%Y%m%d").to_string()).with_param("VALUE", "DATE")
            }
            DtValue::Floating(n) => Prop::new(name, n.format("%Y%m%dT%H%M%S").to_string()),
            DtValue::Utc(u) => Prop::new(name, u.format("%Y%m%dT%H%M%SZ").to_string()),
            DtValue::Zoned(tz, n) => {
                Prop::new(name, n.format("%Y%m%dT%H%M%S").to_string()).with_param("TZID", tz.name())
            }
        }
    }

    pub fn is_date(&self) -> bool {
        matches!(self, DtValue::Date(_))
    }

    /// Wall-clock time in the viewer's zone `view` (all-day values become midnight).
    pub fn local(&self, view: Tz) -> NaiveDateTime {
        match self {
            DtValue::Date(d) => d.and_time(NaiveTime::MIN),
            DtValue::Floating(n) => *n,
            DtValue::Utc(u) => u.with_timezone(&view).naive_local(),
            DtValue::Zoned(tz, n) => match tz.from_local_datetime(n).earliest() {
                Some(t) => t.with_timezone(&view).naive_local(),
                None => *n, // non-existent local time (DST gap)
            },
        }
    }

    pub fn local_date(&self, view: Tz) -> NaiveDate {
        match self {
            DtValue::Date(d) => *d,
            _ => self.local(view).date(),
        }
    }
}

fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let (mut secs, mut num) = (0i64, String::new());
    let mut in_time = false;
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                secs += n * match (c, in_time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(Duration::seconds(if neg { -secs } else { secs }))
}

fn add(dt: &DtValue, d: Duration) -> DtValue {
    match dt {
        DtValue::Date(x) => DtValue::Date(*x + d),
        DtValue::Floating(x) => DtValue::Floating(*x + d),
        DtValue::Utc(x) => DtValue::Utc(*x + d),
        DtValue::Zoned(tz, x) => DtValue::Zoned(*tz, *x + d),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: DtValue,
    pub end: Option<DtValue>,
    pub status: Option<String>,
    pub recurrence_id: Option<DtValue>,
    pub has_rrule: bool,
}

impl Event {
    pub fn from_component(c: &Component) -> Option<Event> {
        let start = DtValue::parse(c.prop("DTSTART")?)?;
        let end = match c.prop("DTEND").and_then(DtValue::parse) {
            Some(e) => Some(e),
            None => c
                .prop("DURATION")
                .and_then(|p| parse_duration(&p.value))
                .map(|d| add(&start, d)),
        };
        Some(Event {
            uid: c.text("UID")?,
            summary: c.text("SUMMARY").unwrap_or_default(),
            description: c.text("DESCRIPTION"),
            location: c.text("LOCATION"),
            start,
            end,
            status: c.text("STATUS"),
            recurrence_id: c.prop("RECURRENCE-ID").and_then(DtValue::parse),
            has_rrule: c.prop("RRULE").is_some(),
        })
    }

    /// Length used for occurrences: DTEND-DTSTART, one day for dates, zero for instants.
    pub fn duration(&self, view: Tz) -> Duration {
        match &self.end {
            Some(e) => e.local(view) - self.start.local(view),
            None if self.start.is_date() => Duration::days(1),
            None => Duration::zero(),
        }
    }
}

/// One concrete instance of an event inside a requested window.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub uid: String,
    pub calendar: String,
    pub href: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    /// Wall-clock in the viewer's zone.
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub recurring: bool,
}

/// Parses the VCALENDAR in `ics` and returns its top-level component list's inner components.
pub fn parse_calendar(ics: &str) -> Result<Component> {
    ical::parse(ics)?
        .into_iter()
        .find(|c| c.name == "VCALENDAR")
        .ok_or_else(|| Error::Ical("no VCALENDAR".into()))
}

fn rrule_text(c: &Component, start: &DtValue) -> Option<String> {
    let rule = c.prop("RRULE")?;
    // The recurrence engine wants DTSTART in text form; dates and floating times are treated
    // as UTC wall-clock (windows are padded and re-filtered in the viewer's zone afterwards).
    let dtstart = match start {
        DtValue::Date(d) => format!("DTSTART:{}T000000Z", d.format("%Y%m%d")),
        DtValue::Floating(n) => format!("DTSTART:{}Z", n.format("%Y%m%dT%H%M%S")),
        DtValue::Utc(u) => format!("DTSTART:{}", u.format("%Y%m%dT%H%M%SZ")),
        DtValue::Zoned(tz, n) => {
            format!("DTSTART;TZID={}:{}", tz.name(), n.format("%Y%m%dT%H%M%S"))
        }
    };
    // For all-day events UNTIL is a DATE; the engine needs a date-time.
    let rule_text: Vec<String> = rule
        .value
        .split(';')
        .map(|kv| match kv.strip_prefix("UNTIL=") {
            Some(u) if start.is_date() && u.len() == 8 => format!("UNTIL={u}T235959Z"),
            _ => kv.to_string(),
        })
        .collect();
    let mut s = format!("{dtstart}\nRRULE:{}", rule_text.join(";"));
    for ex in c.props_named("EXDATE") {
        for v in ex.value.split(',') {
            let dt = DtValue::parse(&Prop {
                value: v.to_string(),
                ..ex.clone()
            })?;
            let line = match dt {
                DtValue::Date(d) => format!("EXDATE:{}T000000Z", d.format("%Y%m%d")),
                DtValue::Floating(n) => format!("EXDATE:{}Z", n.format("%Y%m%dT%H%M%S")),
                DtValue::Utc(u) => format!("EXDATE:{}", u.format("%Y%m%dT%H%M%SZ")),
                DtValue::Zoned(tz, n) => {
                    format!("EXDATE;TZID={}:{}", tz.name(), n.format("%Y%m%dT%H%M%S"))
                }
            };
            s.push('\n');
            s.push_str(&line);
        }
    }
    Some(s)
}

/// Start values of a (possibly recurring) component that fall in `[from, to)` (viewer-zone
/// wall-clock). Non-recurring components yield their own start.
pub fn expand_starts(
    c: &Component,
    start: &DtValue,
    view: Tz,
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> Vec<DtValue> {
    let Some(text) = rrule_text(c, start) else {
        return vec![start.clone()];
    };
    let Ok(set) = text.parse::<rrule::RRuleSet>() else {
        return vec![start.clone()];
    };
    let pad = Duration::days(2);
    let utc = |n: NaiveDateTime| rrule::Tz::UTC.from_utc_datetime(&n);
    let res = set.after(utc(from - pad)).before(utc(to + pad)).all(2000);
    res.dates
        .into_iter()
        .map(|d| {
            // Rebuild a DtValue of the same flavour as the master start.
            match start {
                DtValue::Date(_) => DtValue::Date(d.date_naive()),
                DtValue::Floating(_) => DtValue::Floating(d.naive_utc()),
                DtValue::Utc(_) => DtValue::Utc(d.with_timezone(&Utc)),
                DtValue::Zoned(tz, _) => DtValue::Zoned(*tz, d.with_timezone(tz).naive_local()),
            }
        })
        .filter(|v| {
            let s = v.local(view);
            s < to && s >= from - Duration::days(366)
        })
        .collect()
}

/// Occurrences of all VEVENTs in one calendar object overlapping `[from, to)`.
pub fn event_occurrences(
    cal: &Component,
    calendar: &str,
    href: &str,
    view: Tz,
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> Vec<Occurrence> {
    let comps: Vec<&Component> = cal.children_named("VEVENT").collect();
    let events: Vec<(&Component, Event)> = comps
        .iter()
        .filter_map(|c| Event::from_component(c).map(|e| (*c, e)))
        .collect();
    // Overrides replace the generated instance with the same RECURRENCE-ID.
    let overridden: Vec<(&str, NaiveDateTime)> = events
        .iter()
        .filter_map(|(_, e)| {
            e.recurrence_id
                .as_ref()
                .map(|r| (e.uid.as_str(), r.local(view)))
        })
        .collect();
    let mut out = Vec::new();
    for (c, e) in &events {
        if e.status.as_deref() == Some("CANCELLED") {
            continue;
        }
        let dur = e.duration(view);
        let all_day = e.start.is_date();
        let starts = if e.recurrence_id.is_some() {
            vec![e.start.clone()]
        } else {
            expand_starts(c, &e.start, view, from, to)
        };
        for s in starts {
            let sl = s.local(view);
            if e.recurrence_id.is_none() && overridden.contains(&(e.uid.as_str(), sl)) {
                continue;
            }
            let el = sl + dur;
            // Overlap test; zero-length events count when their start is inside.
            let overlaps = if dur.is_zero() {
                sl >= from && sl < to
            } else {
                sl < to && el > from
            };
            if overlaps {
                out.push(Occurrence {
                    uid: e.uid.clone(),
                    calendar: calendar.to_string(),
                    href: href.to_string(),
                    summary: e.summary.clone(),
                    description: e.description.clone(),
                    location: e.location.clone(),
                    start: sl,
                    end: el,
                    all_day,
                    recurring: e.has_rrule || e.recurrence_id.is_some(),
                });
            }
        }
    }
    out.sort_by_key(|o| (o.start, o.summary.clone()));
    out
}

// ------------------------------------------------------------------------------- tasks

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    NeedsAction,
    InProcess,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::NeedsAction => "NEEDS-ACTION",
            TodoStatus::InProcess => "IN-PROCESS",
            TodoStatus::Completed => "COMPLETED",
            TodoStatus::Cancelled => "CANCELLED",
        }
    }

    pub fn parse(s: &str) -> TodoStatus {
        match s.to_ascii_uppercase().as_str() {
            "IN-PROCESS" => TodoStatus::InProcess,
            "COMPLETED" => TodoStatus::Completed,
            "CANCELLED" => TodoStatus::Cancelled,
            _ => TodoStatus::NeedsAction,
        }
    }
}

/// The task fields Synapse understands; everything else stays in the raw component.
#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: TodoStatus,
    pub percent: Option<u8>,
    /// iCalendar PRIORITY: 0 = none, 1 = highest … 9 = lowest.
    pub priority: u8,
    pub start: Option<DtValue>,
    pub due: Option<DtValue>,
    pub completed: Option<DateTime<Utc>>,
    pub last_modified: Option<DateTime<Utc>>,
    pub has_rrule: bool,
}

fn utc_prop(c: &Component, name: &str) -> Option<DateTime<Utc>> {
    match DtValue::parse(c.prop(name)?)? {
        DtValue::Utc(u) => Some(u),
        DtValue::Floating(n) | DtValue::Zoned(_, n) => Some(Utc.from_utc_datetime(&n)),
        DtValue::Date(d) => Some(Utc.from_utc_datetime(&d.and_time(NaiveTime::MIN))),
    }
}

impl Todo {
    pub fn from_component(c: &Component) -> Option<Todo> {
        Some(Todo {
            uid: c.text("UID")?,
            summary: c.text("SUMMARY").unwrap_or_default(),
            description: c.text("DESCRIPTION").filter(|s| !s.trim().is_empty()),
            status: c
                .text("STATUS")
                .map(|s| TodoStatus::parse(&s))
                .unwrap_or(TodoStatus::NeedsAction),
            percent: c
                .text("PERCENT-COMPLETE")
                .and_then(|s| s.trim().parse().ok()),
            priority: c
                .text("PRIORITY")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            start: c.prop("DTSTART").and_then(DtValue::parse),
            due: c.prop("DUE").and_then(DtValue::parse),
            completed: utc_prop(c, "COMPLETED"),
            last_modified: utc_prop(c, "LAST-MODIFIED").or_else(|| utc_prop(c, "DTSTAMP")),
            has_rrule: c.prop("RRULE").is_some(),
        })
    }

    /// Writes this task's fields onto `c`, keeping unknown properties, and stamps
    /// LAST-MODIFIED/DTSTAMP with `now`.
    pub fn apply_to(&self, c: &mut Component, now: DateTime<Utc>) {
        c.set(Prop::new("UID", &self.uid));
        c.set(Prop::new("SUMMARY", escape(&self.summary)));
        match &self.description {
            Some(d) => c.set(Prop::new("DESCRIPTION", escape(d))),
            None => c.remove("DESCRIPTION"),
        }
        c.set(Prop::new("STATUS", self.status.as_str()));
        match (self.status, self.percent) {
            (TodoStatus::Completed, _) => c.set(Prop::new("PERCENT-COMPLETE", "100")),
            (_, Some(p)) if p < 100 => c.set(Prop::new("PERCENT-COMPLETE", p.to_string())),
            _ => c.remove("PERCENT-COMPLETE"),
        }
        if self.priority == 0 {
            c.remove("PRIORITY");
        } else {
            c.set(Prop::new("PRIORITY", self.priority.to_string()));
        }
        match &self.start {
            Some(s) => c.set(s.to_prop("DTSTART")),
            None => c.remove("DTSTART"),
        }
        match &self.due {
            Some(d) => c.set(d.to_prop("DUE")),
            None => c.remove("DUE"),
        }
        match self.completed {
            Some(t) if self.status == TodoStatus::Completed => {
                c.set(DtValue::Utc(t).to_prop("COMPLETED"))
            }
            _ => c.remove("COMPLETED"),
        }
        let stamp = DtValue::Utc(now);
        c.set(stamp.to_prop("LAST-MODIFIED"));
        c.set(stamp.to_prop("DTSTAMP"));
    }

    /// A fresh VCALENDAR wrapping one VTODO.
    pub fn to_calendar(&self, now: DateTime<Utc>) -> Component {
        let mut cal = Component::new("VCALENDAR");
        cal.props.push(Prop::new("VERSION", "2.0"));
        cal.props
            .push(Prop::new("PRODID", "-//Synapse//CalDAV//EN"));
        let mut t = Component::new("VTODO");
        self.apply_to(&mut t, now);
        cal.children.push(t);
        cal
    }
}

/// First VTODO of a calendar object that is not a recurrence override.
pub fn master_todo(cal: &Component) -> Option<&Component> {
    cal.children_named("VTODO")
        .find(|c| c.prop("RECURRENCE-ID").is_none())
        .or_else(|| cal.children_named("VTODO").next())
}

pub fn master_todo_mut(cal: &mut Component) -> Option<&mut Component> {
    let idx = cal
        .children
        .iter()
        .position(|c| c.name == "VTODO" && c.prop("RECURRENCE-ID").is_none())
        .or_else(|| cal.children.iter().position(|c| c.name == "VTODO"))?;
    Some(&mut cal.children[idx])
}

/// Due/start instances of a recurring task inside `[from, to)`.
pub fn todo_occurrences(
    c: &Component,
    view: Tz,
    from: NaiveDateTime,
    to: NaiveDateTime,
) -> Vec<DtValue> {
    let Some(anchor) = c
        .prop("DTSTART")
        .or_else(|| c.prop("DUE"))
        .and_then(DtValue::parse)
    else {
        return Vec::new();
    };
    expand_starts(c, &anchor, view, from, to)
}

pub fn calendar_kinds(cal: &Component) -> (bool, bool) {
    (
        cal.children.iter().any(|c| c.name == "VEVENT"),
        cal.children.iter().any(|c| c.name == "VTODO"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn weekly_zoned_with_exdate_and_override() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s1\r\nSUMMARY:Standup\r\nDTSTART;TZID=Europe/Madrid:20261007T100000\r\nDTEND;TZID=Europe/Madrid:20261007T103000\r\nRRULE:FREQ=WEEKLY;BYDAY=WE\r\nEXDATE;TZID=Europe/Madrid:20261014T100000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:s1\r\nSUMMARY:Standup (moved)\r\nRECURRENCE-ID;TZID=Europe/Madrid:20261028T100000\r\nDTSTART;TZID=Europe/Madrid:20261028T150000\r\nDTEND;TZID=Europe/Madrid:20261028T153000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let cal = parse_calendar(ics).unwrap();
        let tz = chrono_tz::Europe::Madrid;
        let win = |a: NaiveDate, b: NaiveDate| {
            event_occurrences(
                &cal,
                "c",
                "h",
                tz,
                a.and_time(NaiveTime::MIN),
                b.and_time(NaiveTime::MIN),
            )
        };
        assert_eq!(win(d(2026, 10, 14), d(2026, 10, 15)).len(), 0, "exdate");
        let o = win(d(2026, 10, 21), d(2026, 10, 22));
        assert_eq!(o.len(), 1);
        assert_eq!(
            o[0].start.time(),
            NaiveTime::from_hms_opt(10, 0, 0).unwrap()
        );
        assert!(o[0].recurring);
        // Override moved the 28th to 15:00 and replaced the generated instance.
        let o = win(d(2026, 10, 28), d(2026, 10, 29));
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].summary, "Standup (moved)");
        // Same wall-clock across the DST change (Oct 25 2026): Nov 4 is still 10:00.
        let o = win(d(2026, 11, 4), d(2026, 11, 5));
        assert_eq!(
            o[0].start.time(),
            NaiveTime::from_hms_opt(10, 0, 0).unwrap()
        );
    }

    #[test]
    fn utc_event_shown_in_viewer_zone_and_all_day_daily_count() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:u\r\nSUMMARY:Call\r\nDTSTART:20261021T230000Z\r\nDURATION:PT1H\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Holiday\r\nDTSTART;VALUE=DATE:20261021\r\nRRULE:FREQ=DAILY;COUNT=3;UNTIL=20261110\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let cal = parse_calendar(ics).unwrap();
        let tz = chrono_tz::Europe::Madrid;
        let day = |n: NaiveDate| {
            event_occurrences(
                &cal,
                "c",
                "h",
                tz,
                n.and_time(NaiveTime::MIN),
                (n + Duration::days(1)).and_time(NaiveTime::MIN),
            )
        };
        // 23:00Z is 01:00 on the 22nd in Madrid (CEST, UTC+2).
        assert!(day(d(2026, 10, 21)).iter().all(|o| o.summary != "Call"));
        let o = day(d(2026, 10, 22));
        assert!(
            o.iter()
                .any(|o| o.summary == "Call" && o.end - o.start == Duration::hours(1))
        );
        assert!(o.iter().any(|o| o.summary == "Holiday" && o.all_day));
        assert!(day(d(2026, 10, 24)).is_empty());
    }

    #[test]
    fn todo_roundtrip_preserves_unknown() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nUID:t\r\nSUMMARY:Pay\r\nX-APPLE-SORT-ORDER:5\r\nPRIORITY:1\r\nDUE;VALUE=DATE:20261025\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let mut cal = parse_calendar(ics).unwrap();
        let mut t = Todo::from_component(master_todo(&cal).unwrap()).unwrap();
        assert_eq!(t.priority, 1);
        t.status = TodoStatus::Completed;
        t.completed = Some(Utc.with_ymd_and_hms(2026, 10, 21, 8, 0, 0).unwrap());
        t.apply_to(master_todo_mut(&mut cal).unwrap(), Utc::now());
        let out = ical::serialize(&cal);
        assert!(out.contains("X-APPLE-SORT-ORDER:5"));
        assert!(out.contains("STATUS:COMPLETED") && out.contains("PERCENT-COMPLETE:100"));
        assert!(out.contains("COMPLETED:20261021T080000Z"));
    }
}
