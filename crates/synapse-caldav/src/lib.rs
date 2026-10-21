//! CalDAV events and VTODO tasks, two-way with Synapse. See `docs/caldav.md`.

pub mod calendar;
pub mod dav;
mod error;
pub mod ical;
pub mod mapping;
pub mod store;
pub mod sync;
pub mod xml;

pub use dav::{CalendarInfo, Credentials, DavClient};
pub use error::{Error, Result};
pub use sync::{Config, SyncReport, events_for_day, materialize_day, refresh_calendars, sync};
