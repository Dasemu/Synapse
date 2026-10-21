//! Minimal RFC 5545 content-line parser/serialiser.
//!
//! Components keep every property (including unknown `X-` ones) in order, so a VTODO fetched
//! from a server can be edited and written back without losing data we do not understand.

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    /// Raw (still escaped) value.
    pub value: String,
}

impl Prop {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Prop {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.into(),
        }
    }

    pub fn with_param(mut self, k: &str, v: &str) -> Self {
        self.params.push((k.to_ascii_uppercase(), v.to_string()));
        self
    }

    pub fn param(&self, k: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }

    /// Value with TEXT escapes resolved.
    pub fn text(&self) -> String {
        unescape(&self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Component {
            name: name.to_ascii_uppercase(),
            ..Default::default()
        }
    }

    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn props_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> {
        self.props
            .iter()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(Prop::text)
    }

    /// Replaces the first property of that name (or appends it) and drops duplicates.
    pub fn set(&mut self, prop: Prop) {
        let name = prop.name.clone();
        let mut placed = false;
        let mut out = Vec::with_capacity(self.props.len() + 1);
        for p in self.props.drain(..) {
            if p.name == name {
                if !placed {
                    out.push(prop.clone());
                    placed = true;
                }
            } else {
                out.push(p);
            }
        }
        if !placed {
            out.push(prop);
        }
        self.props = out;
    }

    pub fn remove(&mut self, name: &str) {
        self.props.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> {
        self.children
            .iter()
            .filter(move |c| c.name.eq_ignore_ascii_case(name))
    }
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            ';' => o.push_str("\\;"),
            ',' => o.push_str("\\,"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            c => o.push(c),
        }
    }
    o
}

pub fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => o.push('\n'),
                Some(x) => o.push(x),
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

fn unfold(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !line.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

fn parse_line(line: &str) -> Result<Prop> {
    // name *(;param=value) : value ; colons inside quoted params are not separators.
    let mut in_quotes = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon.ok_or_else(|| Error::Ical(format!("bad content line: {line}")))?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts: Vec<&str> = Vec::new();
    let (mut start, mut q) = (0, false);
    for (i, c) in head.char_indices() {
        match c {
            '"' => q = !q,
            ';' if !q => {
                parts.push(&head[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&head[start..]);
    let mut prop = Prop::new(parts[0], value);
    for p in &parts[1..] {
        if let Some((k, v)) = p.split_once('=') {
            prop.params
                .push((k.to_ascii_uppercase(), v.trim_matches('"').to_string()));
        }
    }
    Ok(prop)
}

/// Parses a calendar object; returns the top-level components (normally one `VCALENDAR`).
pub fn parse(input: &str) -> Result<Vec<Component>> {
    let mut stack: Vec<Component> = Vec::new();
    let mut done = Vec::new();
    for line in unfold(input) {
        let prop = parse_line(&line)?;
        match prop.name.as_str() {
            "BEGIN" => stack.push(Component::new(&prop.value)),
            "END" => {
                let c = stack
                    .pop()
                    .ok_or_else(|| Error::Ical("END without BEGIN".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(c),
                    None => done.push(c),
                }
            }
            _ => match stack.last_mut() {
                Some(c) => c.props.push(prop),
                None => return Err(Error::Ical("property outside component".into())),
            },
        }
    }
    if !stack.is_empty() {
        return Err(Error::Ical("unterminated component".into()));
    }
    Ok(done)
}

fn fold(line: &str, out: &mut String) {
    // Fold at 75 octets without splitting a UTF-8 character.
    let mut count = 0;
    for c in line.chars() {
        let n = c.len_utf8();
        if count + n > 75 {
            out.push_str("\r\n ");
            count = 1;
        }
        out.push(c);
        count += n;
    }
    out.push_str("\r\n");
}

fn write_component(c: &Component, out: &mut String) {
    fold(&format!("BEGIN:{}", c.name), out);
    for p in &c.props {
        let mut l = p.name.clone();
        for (k, v) in &p.params {
            l.push(';');
            l.push_str(k);
            l.push('=');
            if v.contains([';', ':', ',']) {
                l.push('"');
                l.push_str(v);
                l.push('"');
            } else {
                l.push_str(v);
            }
        }
        l.push(':');
        l.push_str(&p.value);
        fold(&l, out);
    }
    for ch in &c.children {
        write_component(ch, out);
    }
    fold(&format!("END:{}", c.name), out);
}

pub fn serialize(c: &Component) -> String {
    let mut s = String::new();
    write_component(c, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_folding_and_params() {
        let src = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Long \r\n title\\, with comma\r\nDTSTART;TZID=\"Europe/Madrid\":20261021T100000\r\nX-FOO;P=\"a:b\":bar\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let cal = parse(src).unwrap();
        let ev = &cal[0].children[0];
        assert_eq!(ev.text("SUMMARY").unwrap(), "Long title, with comma");
        assert_eq!(
            ev.prop("DTSTART").unwrap().param("TZID"),
            Some("Europe/Madrid")
        );
        assert_eq!(ev.prop("X-FOO").unwrap().param("P"), Some("a:b"));
        let again = parse(&serialize(&cal[0])).unwrap();
        assert_eq!(cal, again);
    }
}
