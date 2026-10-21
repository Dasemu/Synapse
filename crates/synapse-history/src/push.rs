//! Push the history branch to any Git smart-HTTP(S) remote.
//!
//! gix cannot push yet, so this speaks the `git-receive-pack` protocol directly: it advertises
//! refs, sends one un-deltified pack with the objects the remote lacks and reads the report.
//! Credentials are used for the HTTP `Authorization` header of this call only; nothing is
//! written to the repository or its config.

use crate::Result;
use crate::error::{Error, git};
use crate::history::History;
use gix::ObjectId;
use std::collections::HashSet;
use std::io::Write;

#[derive(Clone)]
pub struct PushAuth {
    pub username: String,
    /// Password or access token.
    pub secret: String,
}

impl std::fmt::Debug for PushAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushAuth")
            .field("username", &self.username)
            .field("secret", &"***")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    /// Objects sent in the pack.
    pub objects: usize,
    /// False when the remote already had our tip.
    pub updated: bool,
}

fn push_err(msg: impl Into<String>) -> Error {
    Error::Push(msg.into())
}

fn pkt(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(format!("{:04x}", data.len() + 4).as_bytes());
    out.extend_from_slice(data);
}

/// Splits pkt-lines; `None` is a flush packet.
fn parse_pkts(mut b: &[u8]) -> Result<Vec<Option<Vec<u8>>>> {
    let mut out = Vec::new();
    while b.len() >= 4 {
        let n = usize::from_str_radix(std::str::from_utf8(&b[..4]).map_err(git)?, 16)
            .map_err(|_| push_err("bad pkt-line"))?;
        if n == 0 {
            out.push(None);
            b = &b[4..];
        } else {
            if n < 4 || b.len() < n {
                return Err(push_err("truncated pkt-line"));
            }
            out.push(Some(b[4..n].to_vec()));
            b = &b[n..];
        }
    }
    Ok(out)
}

fn basic(auth: &PushAuth) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{}:{}", auth.username, auth.secret);
    let mut out = String::from("Basic ");
    for c in raw.as_bytes().chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl History {
    /// Pushes the history branch to `url` (an `http(s)://` Git remote) as `branch`.
    /// Fast-forward only unless `force`.
    pub fn push(
        &self,
        url: &str,
        branch: &str,
        auth: Option<&PushAuth>,
        force: bool,
    ) -> Result<PushReport> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(push_err("only http(s) remotes are supported"));
        }
        let base = url.trim_end_matches('/');
        let tip = self
            .repo
            .head()?
            .ok_or_else(|| push_err("nothing to push: no snapshots yet"))?;
        let target = format!("refs/heads/{branch}");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();

        // 1. Ref advertisement.
        let mut req = agent.get(&format!("{base}/info/refs?service=git-receive-pack"));
        if let Some(a) = auth {
            req = req.header("Authorization", &basic(a));
        }
        let mut resp = req.call().map_err(|e| push_err(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(push_err(format!("remote answered {}", resp.status())));
        }
        let body = resp
            .body_mut()
            .read_to_vec()
            .map_err(|e| push_err(e.to_string()))?;
        let mut old = ObjectId::null(gix::hash::Kind::Sha1);
        for line in parse_pkts(&body)?.into_iter().flatten() {
            let text = String::from_utf8_lossy(&line);
            let first = text.split('\0').next().unwrap_or("").trim_end();
            if let Some((oid, name)) = first.split_once(' ')
                && name == target
            {
                old = ObjectId::from_hex(oid.as_bytes()).map_err(git)?;
            }
        }
        if old == tip {
            return Ok(PushReport {
                objects: 0,
                updated: false,
            });
        }

        // 2. Objects the remote lacks.
        let mut have = HashSet::new();
        if !old.is_null() {
            if self.repo.repo.find_object(old).is_ok() {
                self.collect(old, &HashSet::new(), &mut have)?;
            } else if !force {
                return Err(push_err("remote has commits we do not have (diverged)"));
            }
            if !force && !self.is_ancestor(old, tip)? {
                return Err(push_err("not a fast-forward (diverged)"));
            }
        }
        let mut send = HashSet::new();
        self.collect(tip, &have, &mut send)?;
        let pack = self.build_pack(&send)?;

        // 3. Send commands + pack.
        let mut payload = Vec::new();
        pkt(
            &mut payload,
            format!("{old} {tip} {target}\0 report-status agent=synapse-history\n").as_bytes(),
        );
        payload.extend_from_slice(b"0000");
        payload.extend_from_slice(&pack);
        let mut req = agent
            .post(&format!("{base}/git-receive-pack"))
            .header("Content-Type", "application/x-git-receive-pack-request");
        if let Some(a) = auth {
            req = req.header("Authorization", &basic(a));
        }
        let mut resp = req.send(&payload).map_err(|e| push_err(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(push_err(format!("remote answered {}", resp.status())));
        }
        let body = resp
            .body_mut()
            .read_to_vec()
            .map_err(|e| push_err(e.to_string()))?;
        let mut unpack_ok = false;
        for line in parse_pkts(&body)?.into_iter().flatten() {
            let t = String::from_utf8_lossy(&line);
            let t = t.trim_end();
            if t == "unpack ok" {
                unpack_ok = true;
            } else if t.starts_with("ng ") || t.starts_with("unpack ") {
                return Err(push_err(format!("rejected: {t}")));
            }
        }
        if !unpack_ok {
            return Err(push_err("remote did not confirm the push"));
        }
        Ok(PushReport {
            objects: send.len(),
            updated: true,
        })
    }

    fn is_ancestor(&self, ancestor: ObjectId, mut cur: ObjectId) -> Result<bool> {
        loop {
            if cur == ancestor {
                return Ok(true);
            }
            match self.repo.parent(cur)? {
                Some(p) => cur = p,
                None => return Ok(false),
            }
        }
    }

    /// Adds every object reachable from `commit` (stopping at `skip`) to `out`.
    fn collect(
        &self,
        commit: ObjectId,
        skip: &HashSet<ObjectId>,
        out: &mut HashSet<ObjectId>,
    ) -> Result<()> {
        let mut cur = Some(commit);
        while let Some(id) = cur {
            if skip.contains(&id) || !out.insert(id) {
                break;
            }
            let c = self
                .repo
                .repo
                .find_object(id)
                .map_err(git)?
                .try_into_commit()
                .map_err(git)?;
            self.collect_tree(c.tree_id().map_err(git)?.detach(), skip, out)?;
            cur = c.parent_ids().next().map(|p| p.detach());
        }
        Ok(())
    }

    fn collect_tree(
        &self,
        tree: ObjectId,
        skip: &HashSet<ObjectId>,
        out: &mut HashSet<ObjectId>,
    ) -> Result<()> {
        if skip.contains(&tree) || !out.insert(tree) {
            return Ok(());
        }
        let obj = self.repo.repo.find_object(tree).map_err(git)?;
        let t = obj.try_into_tree().map_err(git)?;
        for e in t.decode().map_err(git)?.entries.iter() {
            let oid = e.oid.to_owned();
            if e.mode.is_tree() {
                self.collect_tree(oid, skip, out)?;
            } else if !skip.contains(&oid) {
                out.insert(oid);
            }
        }
        Ok(())
    }

    fn build_pack(&self, objects: &HashSet<ObjectId>) -> Result<Vec<u8>> {
        let mut ids: Vec<_> = objects.iter().copied().collect();
        ids.sort();
        let mut pack = Vec::new();
        pack.extend_from_slice(b"PACK");
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&(ids.len() as u32).to_be_bytes());
        for id in ids {
            let obj = self.repo.repo.find_object(id).map_err(git)?;
            let ty: u8 = match obj.kind {
                gix::object::Kind::Commit => 1,
                gix::object::Kind::Tree => 2,
                gix::object::Kind::Blob => 3,
                gix::object::Kind::Tag => 4,
            };
            let mut size = obj.data.len();
            let mut first = (ty << 4) | (size & 0x0f) as u8;
            size >>= 4;
            if size > 0 {
                first |= 0x80;
            }
            pack.push(first);
            while size > 0 {
                let mut b = (size & 0x7f) as u8;
                size >>= 7;
                if size > 0 {
                    b |= 0x80;
                }
                pack.push(b);
            }
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&obj.data)?;
            pack.extend_from_slice(&z.finish()?);
        }
        let mut h = gix::hash::hasher(gix::hash::Kind::Sha1);
        h.update(&pack);
        let digest = h.try_finalize().map_err(git)?;
        pack.extend_from_slice(digest.as_bytes());
        Ok(pack)
    }
}
