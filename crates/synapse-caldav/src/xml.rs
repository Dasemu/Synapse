//! WebDAV multistatus parsing (namespace prefixes are ignored; local names only).

use crate::{Error, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, Default, Clone)]
pub struct Response {
    pub href: String,
    /// Response-level status (used by `sync-collection` for removed members).
    pub status: Option<String>,
    /// Elements found under `prop` in successful propstats: (path below `prop`, text).
    /// Every element yields an entry, so presence can be tested with `has`.
    pub items: Vec<(Vec<String>, String)>,
}

impl Response {
    pub fn text(&self, path: &[&str]) -> Option<&str> {
        self.items
            .iter()
            .find(|(p, t)| p.iter().map(String::as_str).eq(path.iter().copied()) && !t.is_empty())
            .map(|(_, t)| t.as_str())
    }

    pub fn has(&self, path: &[&str]) -> bool {
        self.items
            .iter()
            .any(|(p, _)| p.iter().map(String::as_str).eq(path.iter().copied()))
    }

    pub fn is_removed(&self) -> bool {
        self.status.as_deref().is_some_and(|s| s.contains(" 404"))
    }
}

#[derive(Debug, Default)]
pub struct Multistatus {
    pub responses: Vec<Response>,
    pub sync_token: Option<String>,
}

fn local(s: &str) -> String {
    s.rsplit(':').next().unwrap_or("").to_string()
}

fn xerr(e: impl std::fmt::Display) -> Error {
    Error::Xml(e.to_string())
}

#[derive(Default)]
struct Parser {
    out: Multistatus,
    stack: Vec<String>,
    resp: Option<Response>,
    ps_items: Vec<(Vec<String>, String)>,
    ps_status: String,
    text: String,
}

impl Parser {
    fn in_prop(&self) -> Option<usize> {
        self.stack.iter().position(|s| s == "prop")
    }

    fn start(&mut self, name: String) {
        self.text.clear();
        match name.as_str() {
            "response" => self.resp = Some(Response::default()),
            "propstat" => {
                self.ps_items.clear();
                self.ps_status.clear();
            }
            _ => {}
        }
        self.stack.push(name);
        if let Some(pos) = self.in_prop()
            && self.stack.len() > pos + 1
            && self.resp.is_some()
        {
            self.ps_items
                .push((self.stack[pos + 1..].to_vec(), String::new()));
        }
    }

    fn end(&mut self) {
        let Some(name) = self.stack.last().cloned() else {
            return;
        };
        let t = std::mem::take(&mut self.text);
        let t = t.trim();
        let in_propstat = self.stack.iter().any(|s| s == "propstat");
        match name.as_str() {
            "href" if self.in_prop().is_none() && self.stack.len() == 3 => {
                if let Some(r) = self.resp.as_mut() {
                    r.href = t.to_string();
                }
            }
            "status" if in_propstat => self.ps_status = t.to_string(),
            "status" => {
                if let Some(r) = self.resp.as_mut() {
                    r.status = Some(t.to_string());
                }
            }
            "sync-token" if self.in_prop().is_none() && self.stack.len() == 2 => {
                self.out.sync_token = Some(t.to_string());
            }
            "propstat" => {
                if let Some(r) = self.resp.as_mut()
                    && self.ps_status.contains(" 2")
                {
                    r.items.append(&mut self.ps_items);
                }
                self.ps_items.clear();
            }
            "response" => {
                if let Some(r) = self.resp.take() {
                    self.out.responses.push(r);
                }
            }
            _ => {}
        }
        if let Some(pos) = self.in_prop()
            && self.stack.len() > pos + 1
            && !t.is_empty()
        {
            let path = self.stack[pos + 1..].to_vec();
            if let Some(it) = self
                .ps_items
                .iter_mut()
                .rev()
                .find(|(p, v)| *p == path && v.is_empty())
            {
                it.1 = t.to_string();
            }
        }
        self.stack.pop();
    }
}

fn element_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    let mut name = local(e.name().as_ref());
    // `<comp name="VTODO"/>` is distinguished by its attribute.
    if name == "comp" {
        for a in e.attributes().flatten() {
            if a.key.as_ref() == "name" {
                name = format!("comp:{}", a.value);
            }
        }
    }
    name
}

pub fn parse_multistatus(xml: &str) -> Result<Multistatus> {
    let mut rd = Reader::from_str(xml);
    let mut p = Parser::default();
    loop {
        match rd.read_event().map_err(xerr)? {
            Event::Start(e) => p.start(element_name(&e)),
            Event::Empty(e) => {
                p.start(element_name(&e));
                p.end();
            }
            Event::Text(t) => p
                .text
                .push_str(&Ok::<_, std::convert::Infallible>(t.xml10_content()).map_err(xerr)?),
            Event::CData(c) => p.text.push_str(c.as_ref()),
            Event::GeneralRef(r) => match r.resolve_char_ref() {
                Ok(Some(c)) => p.text.push(c),
                _ => match &*r {
                    "amp" => p.text.push('&'),
                    "lt" => p.text.push('<'),
                    "gt" => p.text.push('>'),
                    "quot" => p.text.push('"'),
                    "apos" => p.text.push('\''),
                    _ => {}
                },
            },
            Event::End(_) => p.end(),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(p.out)
}

/// Escapes text for inclusion in an XML request body.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multistatus_basic() {
        let x = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:response><d:href>/cal/a.ics</d:href><d:propstat><d:prop><d:getetag>"e1"</d:getetag>
<c:calendar-data>BEGIN:VCALENDAR&#13;
SUMMARY:A &amp; B
END:VCALENDAR</c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
<d:propstat><d:prop><d:getctag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
<d:response><d:href>/cal/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
<d:sync-token>tok-7</d:sync-token>
</d:multistatus>"#;
        let m = parse_multistatus(x).unwrap();
        assert_eq!(m.sync_token.as_deref(), Some("tok-7"));
        assert_eq!(m.responses.len(), 2);
        assert_eq!(m.responses[0].href, "/cal/a.ics");
        assert_eq!(m.responses[0].text(&["getetag"]), Some("\"e1\""));
        assert!(
            m.responses[0]
                .text(&["calendar-data"])
                .unwrap()
                .contains("A & B")
        );
        assert!(!m.responses[0].has(&["getctag"]), "404 propstat ignored");
        assert!(m.responses[1].is_removed());
    }
}
