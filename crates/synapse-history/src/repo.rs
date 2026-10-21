//! Thin wrapper over a bare gix repository: whole-tree commits from a `path → blob` map.

use crate::Result;
use crate::error::git;
use gix::ObjectId;
use gix::objs::{Tree, tree};
use std::collections::BTreeMap;
use std::path::Path;

pub const BRANCH: &str = "refs/heads/main";

pub type Files = BTreeMap<String, ObjectId>;

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    /// Seconds since epoch.
    pub time: i64,
    pub message: String,
}

pub struct Repo {
    pub(crate) repo: gix::Repository,
}

impl Repo {
    pub fn open_or_init(dir: &Path) -> Result<Self> {
        let repo = if dir.join("HEAD").exists() {
            gix::open(dir).map_err(git)?
        } else {
            std::fs::create_dir_all(dir)?;
            let repo = gix::init_bare(dir).map_err(git)?;
            std::fs::write(dir.join("HEAD"), format!("ref: {BRANCH}\n"))?;
            repo
        };
        Ok(Self { repo })
    }

    pub fn head(&self) -> Result<Option<ObjectId>> {
        self.repo
            .try_find_reference(BRANCH)
            .map_err(git)?
            .map(|mut r| r.peel_to_id().map(|i| i.detach()))
            .transpose()
            .map_err(git)
    }

    /// All files of a commit's tree, as `path → blob id`.
    pub fn files(&self, commit: ObjectId) -> Result<Files> {
        let c = self
            .repo
            .find_object(commit)
            .map_err(git)?
            .try_into_commit()
            .map_err(git)?;
        let mut out = Files::new();
        self.walk(c.tree_id().map_err(git)?.detach(), "", &mut out)?;
        Ok(out)
    }

    fn walk(&self, tree: ObjectId, prefix: &str, out: &mut Files) -> Result<()> {
        let obj = self.repo.find_object(tree).map_err(git)?;
        let t = obj.try_into_tree().map_err(git)?;
        let decoded = t.decode().map_err(git)?;
        for e in decoded.entries.iter() {
            let name = format!("{prefix}{}", e.filename);
            if e.mode.is_tree() {
                self.walk(e.oid.to_owned(), &format!("{name}/"), out)?;
            } else {
                out.insert(name, e.oid.to_owned());
            }
        }
        Ok(())
    }

    pub fn write_blob(&self, bytes: &[u8]) -> Result<ObjectId> {
        Ok(self.repo.write_blob(bytes).map_err(git)?.detach())
    }

    pub fn read_blob(&self, id: ObjectId) -> Result<Vec<u8>> {
        Ok(self.repo.find_object(id).map_err(git)?.detach().data)
    }

    fn write_tree(&self, files: &Files, dir: &str) -> Result<ObjectId> {
        let mut entries = Vec::new();
        let mut subdirs: BTreeMap<&str, ()> = BTreeMap::new();
        for (path, oid) in files.range(dir.to_string()..) {
            let Some(rest) = path.strip_prefix(dir) else {
                break;
            };
            match rest.split_once('/') {
                None => entries.push(tree::Entry {
                    mode: tree::EntryKind::Blob.into(),
                    filename: rest.into(),
                    oid: *oid,
                }),
                Some((d, _)) => {
                    subdirs.insert(d, ());
                }
            }
        }
        for d in subdirs.keys() {
            let oid = self.write_tree(files, &format!("{dir}{d}/"))?;
            entries.push(tree::Entry {
                mode: tree::EntryKind::Tree.into(),
                filename: (*d).into(),
                oid,
            });
        }
        entries.sort();
        Ok(self
            .repo
            .write_object(Tree { entries })
            .map_err(git)?
            .detach())
    }

    /// Commits the full file set on top of the current branch tip. `time` is epoch seconds.
    pub fn commit(&self, files: &Files, message: &str, time: i64) -> Result<ObjectId> {
        let tree = self.write_tree(files, "")?;
        let parents: Vec<ObjectId> = self.head()?.into_iter().collect();
        let stamp = format!("{time} +0000");
        let sig = gix::actor::SignatureRef {
            name: "Synapse".into(),
            email: "synapse@localhost".into(),
            time: &stamp,
        };
        Ok(self
            .repo
            .commit_as(sig, sig, BRANCH, message, tree, parents)
            .map_err(git)?
            .detach())
    }

    pub fn commit_info(&self, id: ObjectId) -> Result<CommitInfo> {
        let c = self
            .repo
            .find_object(id)
            .map_err(git)?
            .try_into_commit()
            .map_err(git)?;
        Ok(CommitInfo {
            id: id.to_string(),
            time: c.time().map_err(git)?.seconds,
            message: c.message_raw_sloppy().to_string(),
        })
    }

    /// Commits from the branch tip backwards (newest first).
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let mut out = Vec::new();
        let mut cur = self.head()?;
        while let Some(id) = cur {
            if out.len() >= limit {
                break;
            }
            let c = self
                .repo
                .find_object(id)
                .map_err(git)?
                .try_into_commit()
                .map_err(git)?;
            cur = c.parent_ids().next().map(|p| p.detach());
            out.push(self.commit_info(id)?);
        }
        Ok(out)
    }

    pub fn parent(&self, id: ObjectId) -> Result<Option<ObjectId>> {
        let c = self
            .repo
            .find_object(id)
            .map_err(git)?
            .try_into_commit()
            .map_err(git)?;
        Ok(c.parent_ids().next().map(|p| p.detach()))
    }

    pub fn parse_id(s: &str) -> Result<ObjectId> {
        ObjectId::from_hex(s.as_bytes()).map_err(git)
    }
}
