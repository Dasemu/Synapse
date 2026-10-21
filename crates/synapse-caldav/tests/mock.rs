//! A tiny in-process CalDAV server (just enough of RFC 4791/6578 for the client).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const HOME: &str = "/dav/calendars/user/";
pub const PERSONAL: &str = "/dav/calendars/user/personal/";
pub const TASKS: &str = "/dav/calendars/user/tasks/";

#[derive(Default)]
pub struct State {
    pub objects: BTreeMap<String, (String, String)>, // path -> (etag, body)
    pub rev: u64,
    log: Vec<(u64, String, bool)>, // rev, path, deleted
    pub support_sync: bool,
    pub requests: Vec<String>,
    pub puts: Vec<(String, Option<String>)>, // path, if-match
}

#[derive(Clone)]
pub struct Mock {
    pub state: Arc<Mutex<State>>,
    pub base: String,
}

impl Mock {
    pub async fn start() -> Mock {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            support_sync: true,
            ..Default::default()
        }));
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = l.accept().await else {
                    break;
                };
                let st = st.clone();
                tokio::spawn(async move {
                    if let Some(req) = read_request(&mut sock).await {
                        let resp = handle(&st, req);
                        let _ = sock.write_all(resp.as_bytes()).await;
                        let _ = sock.shutdown().await;
                    }
                });
            }
        });
        Mock { state, base }
    }

    /// Server-side write (as another client would do). Returns the new etag.
    pub fn set(&self, path: &str, body: &str) -> String {
        let mut s = self.state.lock().unwrap();
        put(&mut s, path, body)
    }

    pub fn remove(&self, path: &str) {
        let mut s = self.state.lock().unwrap();
        s.objects.remove(path);
        s.rev += 1;
        let r = s.rev;
        s.log.push((r, path.to_string(), true));
    }

    pub fn body(&self, path: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .objects
            .get(path)
            .map(|o| o.1.clone())
    }

    pub fn paths(&self, dir: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .objects
            .keys()
            .filter(|p| p.starts_with(dir))
            .cloned()
            .collect()
    }

    pub fn disable_sync_collection(&self) {
        self.state.lock().unwrap().support_sync = false;
    }

    pub fn count_requests(&self, needle: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.contains(needle))
            .count()
    }
}

fn put(s: &mut State, path: &str, body: &str) -> String {
    s.rev += 1;
    let etag = format!("\"e{}\"", s.rev);
    s.objects
        .insert(path.to_string(), (etag.clone(), body.to_string()));
    let r = s.rev;
    s.log.push((r, path.to_string(), false));
    etag
}

struct Req {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let headers: BTreeMap<String, String> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        })
        .collect();
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

fn http(status: &str, headers: &[(&str, String)], body: &str) -> String {
    let mut s = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn multistatus(inner: &str) -> String {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/">{inner}</d:multistatus>"#
    );
    http(
        "207 Multi-Status",
        &[("Content-Type", "application/xml".into())],
        &body,
    )
}

fn prop_response(href: &str, props: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

fn ctag(s: &State, dir: &str) -> String {
    // Per-collection ctag: highest revision touching the collection.
    let r = s
        .log
        .iter()
        .filter(|(_, p, _)| p.starts_with(dir))
        .map(|(r, _, _)| *r)
        .max()
        .unwrap_or(0);
    format!("ctag-{r}")
}

fn calendar_props(s: &State, dir: &str, name: &str, comps: &[&str]) -> String {
    let comps: String = comps
        .iter()
        .map(|c| format!(r#"<c:comp name="{c}"/>"#))
        .collect();
    format!(
        "<d:displayname>{name}</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set>{comps}</c:supported-calendar-component-set><cs:getctag>{}</cs:getctag>",
        ctag(s, dir)
    )
}

fn hrefs_in(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("<d:href>") {
        let after = &rest[i + 8..];
        let Some(j) = after.find("</d:href>") else {
            break;
        };
        out.push(after[..j].replace("&amp;", "&"));
        rest = &after[j..];
    }
    out
}

fn handle(st: &Arc<Mutex<State>>, req: Req) -> String {
    let mut s = st.lock().unwrap();
    s.requests.push(format!("{} {}", req.method, req.path));
    if !req
        .headers
        .get("authorization")
        .is_some_and(|a| a.starts_with("Basic "))
    {
        return http(
            "401 Unauthorized",
            &[("WWW-Authenticate", "Basic realm=\"mock\"".into())],
            "",
        );
    }
    let (m, p, b) = (req.method.as_str(), req.path.as_str(), req.body.as_str());
    match (m, p) {
        ("PROPFIND", "/.well-known/caldav") => {
            http("301 Moved Permanently", &[("Location", "/dav/".into())], "")
        }
        ("PROPFIND", "/dav/") => multistatus(&prop_response(
            "/dav/",
            "<d:current-user-principal><d:href>/dav/principals/user/</d:href></d:current-user-principal>",
        )),
        ("PROPFIND", "/dav/principals/user/") => multistatus(&prop_response(
            p,
            &format!("<c:calendar-home-set><d:href>{HOME}</d:href></c:calendar-home-set>"),
        )),
        ("PROPFIND", HOME) => {
            let mut inner = prop_response(HOME, "<d:resourcetype><d:collection/></d:resourcetype>");
            inner += &prop_response(
                PERSONAL,
                &calendar_props(&s, PERSONAL, "Personal", &["VEVENT", "VTODO"]),
            );
            inner += &prop_response(TASKS, &calendar_props(&s, TASKS, "Tasks", &["VTODO"]));
            multistatus(&inner)
        }
        ("PROPFIND", cal) if cal == PERSONAL || cal == TASKS => {
            if b.contains("getctag") {
                multistatus(&prop_response(
                    cal,
                    &format!("<cs:getctag>{}</cs:getctag>", ctag(&s, cal)),
                ))
            } else {
                let mut inner =
                    prop_response(cal, "<d:resourcetype><d:collection/></d:resourcetype>");
                for (path, (etag, _)) in s.objects.iter().filter(|(k, _)| k.starts_with(cal)) {
                    inner += &prop_response(
                        path,
                        &format!("<d:getetag>{}</d:getetag>", xml_escape(etag)),
                    );
                }
                multistatus(&inner)
            }
        }
        ("REPORT", cal) if b.contains("sync-collection") => {
            if !s.support_sync {
                return http("501 Not Implemented", &[], "");
            }
            let since: u64 = b
                .split("<d:sync-token>")
                .nth(1)
                .and_then(|t| t.split("</d:sync-token>").next())
                .and_then(|t| t.strip_prefix("tok-"))
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);
            let mut latest: BTreeMap<&String, bool> = BTreeMap::new();
            for (r, path, deleted) in &s.log {
                if *r > since && path.starts_with(cal) {
                    latest.insert(path, *deleted);
                }
            }
            let mut inner = String::new();
            for (path, deleted) in latest {
                if deleted {
                    inner += &format!(
                        "<d:response><d:href>{path}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"
                    );
                } else if let Some((etag, _)) = s.objects.get(path) {
                    inner += &prop_response(
                        path,
                        &format!("<d:getetag>{}</d:getetag>", xml_escape(etag)),
                    );
                }
            }
            inner += &format!("<d:sync-token>tok-{}</d:sync-token>", s.rev);
            multistatus(&inner)
        }
        ("REPORT", _) if b.contains("calendar-multiget") => {
            let mut inner = String::new();
            for h in hrefs_in(b) {
                if let Some((etag, body)) = s.objects.get(&h) {
                    inner += &prop_response(
                        &h,
                        &format!(
                            "<d:getetag>{}</d:getetag><c:calendar-data>{}</c:calendar-data>",
                            xml_escape(etag),
                            xml_escape(body)
                        ),
                    );
                }
            }
            multistatus(&inner)
        }
        ("GET", path) => match s.objects.get(path) {
            Some((etag, body)) => http(
                "200 OK",
                &[
                    ("ETag", etag.clone()),
                    ("Content-Type", "text/calendar".into()),
                ],
                body,
            ),
            None => http("404 Not Found", &[], ""),
        },
        ("PUT", path) => {
            let if_match = req.headers.get("if-match").cloned();
            s.puts.push((path.to_string(), if_match.clone()));
            let current = s.objects.get(path).map(|o| o.0.clone());
            let ok = match (
                &if_match,
                req.headers.get("if-none-match").map(String::as_str),
                &current,
            ) {
                (Some(m), _, Some(c)) => m == c,
                (Some(_), _, None) => false,
                (None, Some("*"), Some(_)) => false,
                _ => true,
            };
            if !ok {
                return http("412 Precondition Failed", &[], "");
            }
            let etag = put(&mut s, path, b);
            http(
                if current.is_some() {
                    "204 No Content"
                } else {
                    "201 Created"
                },
                &[("ETag", etag)],
                "",
            )
        }
        ("DELETE", path) => {
            if let (Some(m), Some((c, _))) = (req.headers.get("if-match"), s.objects.get(path))
                && m != c
            {
                return http("412 Precondition Failed", &[], "");
            }
            if s.objects.remove(path).is_some() {
                s.rev += 1;
                let r = s.rev;
                s.log.push((r, path.to_string(), true));
                http("204 No Content", &[], "")
            } else {
                http("404 Not Found", &[], "")
            }
        }
        _ => http("404 Not Found", &[], ""),
    }
}
