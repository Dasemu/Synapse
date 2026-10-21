//! Content-addressed blob store for assets (images, PDFs, videos...). Files are streamed and
//! hashed with blake3; identical content is stored once at `blobs/<2 hex>/<rest>`.

use crate::Result;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// A blake3 hex digest: 64 lowercase hex chars.
pub fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
    }

    /// Path of a blob. Callers must pass a valid hash (see [`is_valid_hash`]).
    pub fn path_of(&self, hash: &str) -> PathBuf {
        self.root.join(&hash[..2]).join(&hash[2..])
    }

    pub fn contains(&self, hash: &str) -> bool {
        is_valid_hash(hash) && self.path_of(hash).exists()
    }

    /// Streams `reader` into the store. Returns `(hash, size)`.
    pub fn put_reader(&self, mut reader: impl Read) -> Result<(String, u64)> {
        let tmp = self
            .root
            .join("tmp")
            .join(crate::model::new_id().to_string());
        let mut out = io::BufWriter::new(fs::File::create(&tmp)?);
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut size = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            size += n as u64;
        }
        out.flush()?;
        drop(out);
        let hash = hasher.finalize().to_hex().to_string();
        let dest = self.path_of(&hash);
        if dest.exists() {
            fs::remove_file(&tmp)?;
        } else {
            fs::create_dir_all(dest.parent().unwrap())?;
            fs::rename(&tmp, &dest)?;
        }
        Ok((hash, size))
    }

    pub fn put_file(&self, path: &Path) -> Result<(String, u64)> {
        self.put_reader(fs::File::open(path)?)
    }

    pub fn open_blob(&self, hash: &str) -> Result<fs::File> {
        if !is_valid_hash(hash) {
            return Err(crate::Error::Invalid(format!("bad blob hash {hash:?}")));
        }
        Ok(fs::File::open(self.path_of(hash))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let s = BlobStore::open(dir.path()).unwrap();
        let (h1, n) = s.put_reader(&b"hello"[..]).unwrap();
        let (h2, _) = s.put_reader(&b"hello"[..]).unwrap();
        assert_eq!((h1.clone(), n), (h2, 5));
        let mut out = String::new();
        s.open_blob(&h1).unwrap().read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello");
    }
}
