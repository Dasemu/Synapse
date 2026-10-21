//! In-memory exact vector index over int8-quantised, L2-normalised embeddings.
//!
//! Why brute force instead of HNSW: pure Rust, no C++ toolchain, trivially correct, supports
//! deletes/updates in O(1), and int8 rows keep memory small (100k × 384 dims = 38 MB). A scan
//! with 8 independent accumulators auto-vectorises (NEON/SSE/AVX) and stays within a few tens
//! of milliseconds at 100k vectors. Beyond ~500k vectors an ANN structure becomes worthwhile.

use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct VecIndex {
    dims: usize,
    ids: Vec<Uuid>,
    pages: Vec<Uuid>,
    stamps: Vec<i64>,
    scales: Vec<f32>,
    data: Vec<i8>,
    slot: HashMap<Uuid, usize>,
}

pub(crate) fn normalize(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n == 0.0 {
        v.to_vec()
    } else {
        v.iter().map(|x| x / n).collect()
    }
}

/// Quantises an (already normalised) vector to int8 with a per-vector scale.
pub(crate) fn quantize(v: &[f32]) -> (Vec<i8>, f32) {
    let max = v.iter().fold(0f32, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return (vec![0; v.len()], 0.0);
    }
    let scale = max / 127.0;
    (v.iter().map(|x| (x / scale).round() as i8).collect(), scale)
}

fn dot(row: &[i8], q: &[f32]) -> f32 {
    let mut acc = [0f32; 8];
    let mut rc = row.chunks_exact(8);
    let mut qc = q.chunks_exact(8);
    for (r, c) in (&mut rc).zip(&mut qc) {
        for i in 0..8 {
            acc[i] += r[i] as f32 * c[i];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for (r, c) in rc.remainder().iter().zip(qc.remainder()) {
        s += *r as f32 * c;
    }
    s
}

impl VecIndex {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn upsert_quantized(&mut self, id: Uuid, page: Uuid, stamp: i64, q: &[i8], scale: f32) {
        if self.dims == 0 {
            self.dims = q.len();
        }
        if q.len() != self.dims {
            return;
        }
        if let Some(&s) = self.slot.get(&id) {
            self.pages[s] = page;
            self.stamps[s] = stamp;
            self.scales[s] = scale;
            self.data[s * self.dims..(s + 1) * self.dims].copy_from_slice(q);
        } else {
            self.slot.insert(id, self.ids.len());
            self.ids.push(id);
            self.pages.push(page);
            self.stamps.push(stamp);
            self.scales.push(scale);
            self.data.extend_from_slice(q);
        }
    }

    pub fn remove(&mut self, id: Uuid) {
        let Some(s) = self.slot.remove(&id) else {
            return;
        };
        let last = self.ids.len() - 1;
        if s != last {
            let moved = self.ids[last];
            self.ids.swap(s, last);
            self.pages.swap(s, last);
            self.stamps.swap(s, last);
            self.scales.swap(s, last);
            for i in 0..self.dims {
                self.data.swap(s * self.dims + i, last * self.dims + i);
            }
            self.slot.insert(moved, s);
        }
        self.ids.pop();
        self.pages.pop();
        self.stamps.pop();
        self.scales.pop();
        self.data.truncate(last * self.dims);
    }

    /// Dequantised vector of `id`.
    pub fn get(&self, id: Uuid) -> Option<Vec<f32>> {
        let s = *self.slot.get(&id)?;
        let sc = self.scales[s];
        Some(
            self.data[s * self.dims..(s + 1) * self.dims]
                .iter()
                .map(|&x| x as f32 * sc)
                .collect(),
        )
    }

    /// Top-`k` by cosine similarity; `query` must be normalised.
    pub fn top_k(&self, query: &[f32], k: usize, page: Option<Uuid>) -> Vec<Neighbor> {
        if query.len() != self.dims || self.ids.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(f32, usize)> = Vec::with_capacity(self.ids.len());
        for s in 0..self.ids.len() {
            if let Some(p) = page
                && self.pages[s] != p
            {
                continue;
            }
            let row = &self.data[s * self.dims..(s + 1) * self.dims];
            scored.push((dot(row, query) * self.scales[s], s));
        }
        let cmp = |a: &(f32, usize), b: &(f32, usize)| b.0.total_cmp(&a.0);
        if scored.len() > k {
            scored.select_nth_unstable_by(k - 1, cmp);
            scored.truncate(k);
        }
        scored.sort_by(cmp);
        scored
            .into_iter()
            .map(|(score, s)| Neighbor {
                id: self.ids[s],
                page: self.pages[s],
                stamp: self.stamps[s],
                score,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Neighbor {
    pub id: Uuid,
    pub page: Uuid,
    pub stamp: i64,
    pub score: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_and_remove() {
        let mut ix = VecIndex::default();
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::now_v7()).collect();
        let p = Uuid::now_v7();
        for (i, v) in [[1.0, 0.0], [0.0, 1.0], [0.7, 0.7]].iter().enumerate() {
            let (q, s) = quantize(&normalize(v));
            ix.upsert_quantized(ids[i], p, 0, &q, s);
        }
        let r = ix.top_k(&normalize(&[1.0, 0.1]), 2, None);
        assert_eq!(r[0].id, ids[0]);
        assert_eq!(r[1].id, ids[2]);
        ix.remove(ids[0]);
        assert_eq!(ix.len(), 2);
        let r = ix.top_k(&normalize(&[1.0, 0.1]), 1, None);
        assert_eq!(r[0].id, ids[2]);
        assert!(ix.get(ids[1]).is_some());
    }
}
