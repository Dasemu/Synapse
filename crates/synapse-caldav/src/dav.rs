//! Async CalDAV client (RFC 4791 / RFC 6578). Credentials are supplied by the caller and held
//! only in memory.

use crate::xml::{Multistatus, esc, parse_multistatus};
use crate::{Error, Result};
use reqwest::header::{
    CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, IF_MATCH, IF_NONE_MATCH, LOCATION,
};
use reqwest::{Method, StatusCode, Url};
use std::sync::Arc;

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    /// Account password or (preferably) an app password.
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarInfo {
    /// Server-relative path, as returned by the server (identity of the collection).
    pub href: String,
    pub name: String,
    pub supports_events: bool,
    pub supports_tasks: bool,
    pub ctag: Option<String>,
    pub sync_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteObject {
    pub href: String,
    pub etag: String,
    pub ics: String,
}

#[derive(Debug, Default)]
pub struct SyncChanges {
    /// (href, etag) of created/modified members.
    pub changed: Vec<(String, String)>,
    pub deleted: Vec<String>,
    pub new_token: Option<String>,
}

pub struct DavClient {
    http: reqwest::Client,
    base: Url,
    creds: Credentials,
}

const NS: &str = r#"xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/""#;

fn tls_client() -> Result<reqwest::Client> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::Tls(e.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(reqwest::Client::builder()
        .use_preconfigured_tls(cfg)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("synapse-caldav")
        .build()?)
}

fn strip_quotes_etag(h: &HeaderMap) -> Option<String> {
    h.get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

impl DavClient {
    /// `base` is the server root (`https://cloud.example.com`) or any DAV URL below it.
    pub fn new(base: &str, creds: Credentials) -> Result<Self> {
        Ok(DavClient {
            http: tls_client()?,
            base: Url::parse(base).map_err(|e| Error::Discovery(format!("bad url: {e}")))?,
            creds,
        })
    }

    fn url(&self, href: &str) -> Result<Url> {
        self.base
            .join(href)
            .map_err(|e| Error::Other(format!("bad href {href}: {e}")))
    }

    /// Sends a request, following redirects manually so the method and body are preserved
    /// (credentials only ever go to the original host).
    async fn send(
        &self,
        method: &str,
        url: Url,
        headers: &[(&'static str, String)],
        body: Option<String>,
    ) -> Result<(StatusCode, HeaderMap, String)> {
        let m = Method::from_bytes(method.as_bytes()).map_err(|e| Error::Other(e.to_string()))?;
        let mut url = url;
        for _ in 0..6 {
            let mut rb = self.http.request(m.clone(), url.clone());
            if url.host_str() == self.base.host_str()
                && url.port_or_known_default() == self.base.port_or_known_default()
            {
                rb = rb.basic_auth(&self.creds.username, Some(&self.creds.password));
            }
            for (k, v) in headers {
                rb = rb.header(
                    *k,
                    HeaderValue::from_str(v).map_err(|e| Error::Other(e.to_string()))?,
                );
            }
            if let Some(b) = &body {
                rb = rb.body(b.clone());
            }
            let resp = rb.send().await?;
            let status = resp.status();
            if status.is_redirection()
                && let Some(loc) = resp.headers().get(LOCATION).and_then(|v| v.to_str().ok())
            {
                url = url.join(loc).map_err(|e| Error::Other(e.to_string()))?;
                continue;
            }
            let h = resp.headers().clone();
            let text = resp.text().await?;
            return Ok((status, h, text));
        }
        Err(Error::Other("too many redirects".into()))
    }

    async fn xml(
        &self,
        method: &str,
        href: &str,
        depth: Option<&str>,
        body: String,
    ) -> Result<(StatusCode, Multistatus)> {
        let mut hs = vec![(
            CONTENT_TYPE.as_str(),
            "application/xml; charset=utf-8".to_string(),
        )];
        if let Some(d) = depth {
            hs.push(("depth", d.to_string()));
        }
        let url = self.url(href)?;
        let (st, _, text) = self.send(method, url, &hs, Some(body)).await?;
        if st.as_u16() == 207 {
            Ok((st, parse_multistatus(&text)?))
        } else {
            Ok((st, Multistatus::default()))
        }
    }

    fn check(st: StatusCode, what: &str) -> Result<()> {
        if st.as_u16() == 207 || st.is_success() {
            Ok(())
        } else {
            Err(Error::Status {
                status: st.as_u16(),
                what: what.to_string(),
            })
        }
    }

    // ------------------------------------------------------------------ discovery

    /// Resolves the calendar-home-set path: `.well-known/caldav`, current-user-principal,
    /// calendar-home-set.
    pub async fn discover(&self) -> Result<String> {
        let principal_body = format!(
            r#"<?xml version="1.0"?><d:propfind {NS}><d:prop><d:current-user-principal/></d:prop></d:propfind>"#
        );
        let mut candidates = vec!["/.well-known/caldav".to_string()];
        let base_path = self.base.path().to_string();
        if base_path != "/" && !base_path.is_empty() {
            candidates.push(base_path);
        }
        candidates.push("/".to_string());
        let mut principal = None;
        let mut last = String::from("no candidate answered");
        for c in &candidates {
            let (st, ms) = self
                .xml("PROPFIND", c, Some("0"), principal_body.clone())
                .await?;
            if st.as_u16() != 207 {
                last = format!("{c}: HTTP {st}");
                continue;
            }
            if let Some(h) = ms
                .responses
                .iter()
                .find_map(|r| r.text(&["current-user-principal", "href"]))
            {
                principal = Some(h.to_string());
                break;
            }
            last = format!("{c}: no current-user-principal");
        }
        let principal = principal.ok_or(Error::Discovery(last))?;
        let home_body = format!(
            r#"<?xml version="1.0"?><d:propfind {NS}><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#
        );
        let (st, ms) = self
            .xml("PROPFIND", &principal, Some("0"), home_body)
            .await?;
        Self::check(st, "calendar-home-set")?;
        ms.responses
            .iter()
            .find_map(|r| r.text(&["calendar-home-set", "href"]))
            .map(str::to_string)
            .ok_or_else(|| Error::Discovery("no calendar-home-set".into()))
    }

    /// Calendar collections below `home` that hold events and/or tasks.
    pub async fn list_calendars(&self, home: &str) -> Result<Vec<CalendarInfo>> {
        let body = format!(
            r#"<?xml version="1.0"?><d:propfind {NS}><d:prop><d:displayname/><d:resourcetype/><c:supported-calendar-component-set/><cs:getctag/><d:sync-token/></d:prop></d:propfind>"#
        );
        let (st, ms) = self.xml("PROPFIND", home, Some("1"), body).await?;
        Self::check(st, "list calendars")?;
        let mut out = Vec::new();
        for r in ms.responses {
            if !r.has(&["resourcetype", "calendar"]) {
                continue;
            }
            let (ev, td) = (
                r.has(&["supported-calendar-component-set", "comp:VEVENT"]),
                r.has(&["supported-calendar-component-set", "comp:VTODO"]),
            );
            // Servers omitting the set support everything.
            let none = !r.has(&["supported-calendar-component-set"]);
            let name = r
                .text(&["displayname"])
                .map(str::to_string)
                .unwrap_or_else(|| {
                    r.href
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string()
                });
            out.push(CalendarInfo {
                name,
                supports_events: ev || none,
                supports_tasks: td || none,
                ctag: r.text(&["getctag"]).map(str::to_string),
                sync_token: r.text(&["sync-token"]).map(str::to_string),
                href: r.href,
            });
        }
        Ok(out)
    }

    // ----------------------------------------------------------------------- sync

    /// RFC 6578 incremental sync. Returns `Ok(None)` when the server does not support it or
    /// rejected the token; callers then fall back to [`DavClient::list_etags`].
    pub async fn sync_collection(
        &self,
        cal: &str,
        token: Option<&str>,
    ) -> Result<Option<SyncChanges>> {
        let body = format!(
            r#"<?xml version="1.0"?><d:sync-collection {NS}><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
            esc(token.unwrap_or(""))
        );
        let (st, ms) = self.xml("REPORT", cal, None, body).await?;
        if matches!(st.as_u16(), 403 | 405 | 409 | 410 | 412 | 415 | 501) {
            return Ok(None);
        }
        Self::check(st, "sync-collection")?;
        let Some(new_token) = ms.sync_token else {
            return Ok(None);
        };
        let mut ch = SyncChanges {
            new_token: Some(new_token),
            ..Default::default()
        };
        for r in ms.responses {
            if r.is_removed() {
                ch.deleted.push(r.href);
            } else if let Some(e) = r.text(&["getetag"]) {
                ch.changed.push((r.href.clone(), e.to_string()));
            }
        }
        Ok(Some(ch))
    }

    /// All members of a collection with their etags (fallback when no sync-collection).
    pub async fn list_etags(&self, cal: &str) -> Result<Vec<(String, String)>> {
        let body = format!(
            r#"<?xml version="1.0"?><d:propfind {NS}><d:prop><d:getetag/></d:prop></d:propfind>"#
        );
        let (st, ms) = self.xml("PROPFIND", cal, Some("1"), body).await?;
        Self::check(st, "list etags")?;
        let me = cal.trim_end_matches('/');
        Ok(ms
            .responses
            .into_iter()
            .filter(|r| r.href.trim_end_matches('/') != me)
            .filter_map(|r| {
                r.text(&["getetag"])
                    .map(|e| (r.href.clone(), e.to_string()))
            })
            .collect())
    }

    /// Current ctag of a collection.
    pub async fn ctag(&self, cal: &str) -> Result<Option<String>> {
        let body = format!(
            r#"<?xml version="1.0"?><d:propfind {NS}><d:prop><cs:getctag/></d:prop></d:propfind>"#
        );
        let (st, ms) = self.xml("PROPFIND", cal, Some("0"), body).await?;
        Self::check(st, "ctag")?;
        Ok(ms
            .responses
            .iter()
            .find_map(|r| r.text(&["getctag"]).map(str::to_string)))
    }

    pub async fn multiget(&self, cal: &str, hrefs: &[String]) -> Result<Vec<RemoteObject>> {
        let mut out = Vec::new();
        for chunk in hrefs.chunks(50) {
            let items: String = chunk
                .iter()
                .map(|h| format!("<d:href>{}</d:href>", esc(h)))
                .collect();
            let body = format!(
                r#"<?xml version="1.0"?><c:calendar-multiget {NS}><d:prop><d:getetag/><c:calendar-data/></d:prop>{items}</c:calendar-multiget>"#
            );
            let (st, ms) = self.xml("REPORT", cal, Some("1"), body).await?;
            Self::check(st, "calendar-multiget")?;
            for r in ms.responses {
                if let (Some(e), Some(d)) = (r.text(&["getetag"]), r.text(&["calendar-data"])) {
                    out.push(RemoteObject {
                        href: r.href.clone(),
                        etag: e.to_string(),
                        ics: d.to_string(),
                    });
                }
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------------ resources

    pub async fn get(&self, href: &str) -> Result<RemoteObject> {
        let (st, h, text) = self.send("GET", self.url(href)?, &[], None).await?;
        Self::check(st, &format!("GET {href}"))?;
        Ok(RemoteObject {
            href: href.to_string(),
            etag: strip_quotes_etag(&h).unwrap_or_default(),
            ics: text,
        })
    }

    /// Creates (`if_match = None`, only if absent) or replaces (`Some(etag)`) an object.
    /// Returns the new etag. A 412 becomes [`Error::PreconditionFailed`].
    pub async fn put(&self, href: &str, ics: &str, if_match: Option<&str>) -> Result<String> {
        let hs = vec![
            (
                CONTENT_TYPE.as_str(),
                "text/calendar; charset=utf-8".to_string(),
            ),
            match if_match {
                Some(e) => (IF_MATCH.as_str(), e.to_string()),
                None => (IF_NONE_MATCH.as_str(), "*".to_string()),
            },
        ];
        let (st, h, _) = self
            .send("PUT", self.url(href)?, &hs, Some(ics.to_string()))
            .await?;
        if st == StatusCode::PRECONDITION_FAILED {
            return Err(Error::PreconditionFailed(href.to_string()));
        }
        Self::check(st, &format!("PUT {href}"))?;
        match strip_quotes_etag(&h) {
            Some(e) => Ok(e),
            // Some servers rewrite the object on PUT and omit the etag.
            None => Ok(self.get(href).await?.etag),
        }
    }

    pub async fn delete(&self, href: &str, if_match: Option<&str>) -> Result<()> {
        let hs: Vec<(&'static str, String)> = if_match
            .map(|e| vec![(IF_MATCH.as_str(), e.to_string())])
            .unwrap_or_default();
        let (st, _, _) = self.send("DELETE", self.url(href)?, &hs, None).await?;
        if st == StatusCode::PRECONDITION_FAILED {
            return Err(Error::PreconditionFailed(href.to_string()));
        }
        if st == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::check(st, &format!("DELETE {href}"))
    }
}
