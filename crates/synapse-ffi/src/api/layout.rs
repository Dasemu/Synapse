//! Force-directed layout (d3-force style): many-body repulsion approximated with a Barnes-Hut
//! quadtree (O(n log n)), springs on edges, a weak pull to the origin and velocity damping
//! with a cooling `alpha`. Dart owns one [`ForceLayout`] per view and calls [`ForceLayout::step`]
//! repeatedly; each call runs on a Rust worker thread, so the UI never blocks.

use super::dto::LayoutFrame;
use flutter_rust_bridge::frb;

const THETA2: f32 = 0.81; // Barnes-Hut opening criterion θ = 0.9, squared.
const ALPHA_MIN: f32 = 0.001;
const ALPHA_DECAY: f32 = 0.0228; // ≈ 300 iterations from 1 to ALPHA_MIN.
const VELOCITY_DECAY: f32 = 0.4;
const CENTER_STRENGTH: f32 = 0.02;
const MAX_LEAF_DEPTH: u32 = 24;

#[frb(opaque)]
pub struct ForceLayout {
    pos: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
    pinned: Vec<Option<[f32; 2]>>,
    /// (source, target, rest length, strength, bias)
    links: Vec<(u32, u32, f32, f32, f32)>,
    charge: f32,
    alpha: f32,
    alpha_target: f32,
    tree: Vec<Cell>,
}

#[derive(Clone, Copy)]
struct Cell {
    x0: f32,
    y0: f32,
    size: f32,
    /// Centre of mass and mass (number of bodies).
    cx: f32,
    cy: f32,
    mass: f32,
    /// Index of the first of four children, or 0 for a leaf.
    child: u32,
    /// Body stored in a leaf (`u32::MAX` when empty or internal).
    body: u32,
}

impl Cell {
    fn new(x0: f32, y0: f32, size: f32) -> Self {
        Cell {
            x0,
            y0,
            size,
            cx: 0.0,
            cy: 0.0,
            mass: 0.0,
            child: 0,
            body: u32::MAX,
        }
    }
}

impl ForceLayout {
    /// A layout for `node_count` nodes and edges `sources[i] → targets[i]` (weights scale the
    /// spring strength). `initial` positions (`x0, y0, ...`) are reused when given, e.g. to keep
    /// the picture stable after filtering; otherwise nodes start on a phyllotaxis spiral.
    #[frb(sync)]
    pub fn new(
        node_count: u32,
        sources: Vec<u32>,
        targets: Vec<u32>,
        weights: Vec<f32>,
        initial: Option<Vec<f32>>,
    ) -> ForceLayout {
        let n = node_count as usize;
        let mut pos = Vec::with_capacity(n);
        for i in 0..n {
            match initial.as_ref() {
                Some(p) if p.len() >= 2 * n && p[2 * i].is_finite() && p[2 * i + 1].is_finite() => {
                    pos.push([p[2 * i], p[2 * i + 1]])
                }
                _ => {
                    // d3's phyllotaxis arrangement: even density, deterministic.
                    let r = 10.0 * (0.5 + i as f32).sqrt();
                    let a = i as f32 * std::f32::consts::PI * (3.0 - 5f32.sqrt());
                    pos.push([r * a.cos(), r * a.sin()]);
                }
            }
        }
        let mut degree = vec![0u32; n];
        let edges: Vec<(u32, u32, f32)> = sources
            .iter()
            .zip(&targets)
            .enumerate()
            .filter(|(_, (s, t))| (**s as usize) < n && (**t as usize) < n && s != t)
            .map(|(e, (&s, &t))| (s, t, weights.get(e).copied().unwrap_or(1.0)))
            .collect();
        for &(s, t, _) in &edges {
            degree[s as usize] += 1;
            degree[t as usize] += 1;
        }
        let links = edges
            .iter()
            .map(|&(s, t, w)| {
                let (ds, dt) = (degree[s as usize] as f32, degree[t as usize] as f32);
                let strength = w.clamp(0.2, 3.0) / ds.min(dt).max(1.0);
                (s, t, 34.0, strength, ds / (ds + dt))
            })
            .collect();
        ForceLayout {
            pos,
            vel: vec![[0.0; 2]; n],
            pinned: vec![None; n],
            links,
            charge: -45.0,
            alpha: if initial.is_some() { 0.3 } else { 1.0 },
            alpha_target: 0.0,
            tree: Vec::new(),
        }
    }

    /// Runs up to `iterations` ticks (stops early once settled).
    pub fn step(&mut self, iterations: u32) -> LayoutFrame {
        for _ in 0..iterations {
            if self.alpha < ALPHA_MIN && self.alpha_target == 0.0 {
                break;
            }
            self.tick();
        }
        LayoutFrame {
            positions: self.pos.iter().flat_map(|p| [p[0], p[1]]).collect(),
            alpha: if self.alpha < ALPHA_MIN {
                0.0
            } else {
                self.alpha
            },
        }
    }

    /// Holds node `index` at `(x, y)` (while dragging) and keeps the layout warm.
    pub fn pin(&mut self, index: u32, x: f32, y: f32) {
        if let Some(p) = self.pinned.get_mut(index as usize) {
            *p = Some([x, y]);
            self.pos[index as usize] = [x, y];
            self.alpha_target = 0.3;
            self.alpha = self.alpha.max(0.3);
        }
    }

    /// Releases a pinned node and lets the layout cool down again.
    pub fn unpin(&mut self, index: u32) {
        if let Some(p) = self.pinned.get_mut(index as usize) {
            *p = None;
        }
        self.alpha_target = 0.0;
    }

    /// Restarts the simulation at temperature `alpha` (0..1).
    pub fn reheat(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    fn tick(&mut self) {
        self.alpha += (self.alpha_target - self.alpha) * ALPHA_DECAY;
        let alpha = self.alpha;
        // Springs.
        for &(s, t, len, strength, bias) in &self.links {
            let (s, t) = (s as usize, t as usize);
            let mut dx = self.pos[t][0] + self.vel[t][0] - self.pos[s][0] - self.vel[s][0];
            let mut dy = self.pos[t][1] + self.vel[t][1] - self.pos[s][1] - self.vel[s][1];
            if dx == 0.0 && dy == 0.0 {
                dx = 1e-3 * (s as f32 - t as f32).signum();
                dy = 1e-3;
            }
            let l = (dx * dx + dy * dy).sqrt();
            let k = (l - len) / l * alpha * strength;
            dx *= k;
            dy *= k;
            self.vel[t][0] -= dx * bias;
            self.vel[t][1] -= dy * bias;
            self.vel[s][0] += dx * (1.0 - bias);
            self.vel[s][1] += dy * (1.0 - bias);
        }
        // Many-body repulsion (Barnes-Hut).
        self.build_tree();
        for i in 0..self.pos.len() {
            let (fx, fy) = self.repulsion(i);
            self.vel[i][0] += fx * alpha;
            self.vel[i][1] += fy * alpha;
        }
        // Centre pull, damping, integration, pins.
        for i in 0..self.pos.len() {
            self.vel[i][0] -= self.pos[i][0] * CENTER_STRENGTH * alpha;
            self.vel[i][1] -= self.pos[i][1] * CENTER_STRENGTH * alpha;
            if let Some(p) = self.pinned[i] {
                self.pos[i] = p;
                self.vel[i] = [0.0; 2];
                continue;
            }
            self.vel[i][0] *= 1.0 - VELOCITY_DECAY;
            self.vel[i][1] *= 1.0 - VELOCITY_DECAY;
            self.pos[i][0] += self.vel[i][0];
            self.pos[i][1] += self.vel[i][1];
        }
    }

    fn build_tree(&mut self) {
        self.tree.clear();
        if self.pos.is_empty() {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &self.pos {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let size = (x1 - x0).max(y1 - y0).max(1.0) * 1.0001;
        self.tree.push(Cell::new(x0, y0, size));
        for i in 0..self.pos.len() {
            self.insert(0, i as u32, 0);
        }
        self.accumulate(0);
    }

    fn quadrant(c: &Cell, p: [f32; 2]) -> u32 {
        let h = c.size / 2.0;
        (if p[0] >= c.x0 + h { 1 } else { 0 }) + (if p[1] >= c.y0 + h { 2 } else { 0 })
    }

    fn split(&mut self, cell: usize) {
        let c = self.tree[cell];
        let h = c.size / 2.0;
        let first = self.tree.len() as u32;
        for q in 0..4 {
            let x = c.x0 + if q & 1 == 1 { h } else { 0.0 };
            let y = c.y0 + if q & 2 == 2 { h } else { 0.0 };
            self.tree.push(Cell::new(x, y, h));
        }
        self.tree[cell].child = first;
    }

    fn insert(&mut self, cell: usize, body: u32, depth: u32) {
        let c = self.tree[cell];
        if c.child == 0 {
            if c.body == u32::MAX && c.mass == 0.0 {
                self.tree[cell].body = body;
                self.tree[cell].mass = 1.0;
                return;
            }
            if depth >= MAX_LEAF_DEPTH {
                // Coincident points: keep them in one leaf, they are handled by mass.
                self.tree[cell].mass += 1.0;
                return;
            }
            let old = c.body;
            self.tree[cell].body = u32::MAX;
            self.tree[cell].mass = 0.0;
            self.split(cell);
            if old != u32::MAX {
                let q = Self::quadrant(&self.tree[cell], self.pos[old as usize]);
                let child = (self.tree[cell].child + q) as usize;
                self.insert(child, old, depth + 1);
            }
        }
        let q = Self::quadrant(&self.tree[cell], self.pos[body as usize]);
        let child = (self.tree[cell].child + q) as usize;
        self.insert(child, body, depth + 1);
    }

    fn accumulate(&mut self, cell: usize) {
        let c = self.tree[cell];
        if c.child == 0 {
            if c.body != u32::MAX {
                let p = self.pos[c.body as usize];
                self.tree[cell].cx = p[0];
                self.tree[cell].cy = p[1];
            }
            return;
        }
        let (mut m, mut x, mut y) = (0.0, 0.0, 0.0);
        for q in 0..4 {
            let k = (c.child + q) as usize;
            self.accumulate(k);
            let ch = self.tree[k];
            m += ch.mass;
            x += ch.cx * ch.mass;
            y += ch.cy * ch.mass;
        }
        self.tree[cell].mass = m;
        if m > 0.0 {
            self.tree[cell].cx = x / m;
            self.tree[cell].cy = y / m;
        }
    }

    fn repulsion(&self, i: usize) -> (f32, f32) {
        let p = self.pos[i];
        let (mut fx, mut fy) = (0.0, 0.0);
        let mut stack = vec![0usize];
        while let Some(k) = stack.pop() {
            let c = &self.tree[k];
            if c.mass == 0.0 {
                continue;
            }
            let dx = c.cx - p[0];
            let dy = c.cy - p[1];
            let mut d2 = dx * dx + dy * dy;
            let far = c.child != 0 && c.size * c.size < THETA2 * d2;
            if c.child == 0 || far {
                if c.child == 0 && c.body == i as u32 && c.mass <= 1.0 {
                    continue;
                }
                if d2 < 1.0 {
                    // Jiggle overlapping nodes apart deterministically.
                    d2 = 1.0;
                }
                let mass = if c.child == 0 && c.body == i as u32 {
                    c.mass - 1.0
                } else {
                    c.mass
                };
                let w = self.charge * mass / d2;
                let (dx, dy) = if dx == 0.0 && dy == 0.0 {
                    let a = i as f32 * 2.399_963;
                    (a.cos(), a.sin())
                } else {
                    (dx, dy)
                };
                fx += dx * w;
                fy += dy * w;
            } else {
                for q in 0..4 {
                    stack.push((c.child + q) as usize);
                }
            }
        }
        (fx, fy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(p: &[f32], a: usize, b: usize) -> f32 {
        ((p[2 * a] - p[2 * b]).powi(2) + (p[2 * a + 1] - p[2 * b + 1]).powi(2)).sqrt()
    }

    #[test]
    fn linked_nodes_end_closer_than_unlinked_and_layout_settles() {
        // Two triangles joined by nothing.
        let s = vec![0, 1, 2, 3, 4, 5];
        let t = vec![1, 2, 0, 4, 5, 3];
        let mut l = ForceLayout::new(6, s, t, vec![1.0; 6], None);
        let mut f = l.step(50);
        for _ in 0..20 {
            if f.alpha == 0.0 {
                break;
            }
            f = l.step(50);
        }
        assert_eq!(f.alpha, 0.0, "settles");
        let p = &f.positions;
        assert!(p.iter().all(|v| v.is_finite()));
        assert!(dist(p, 0, 1) < dist(p, 0, 3));
        assert!(dist(p, 3, 4) < dist(p, 2, 5));
    }

    #[test]
    fn pins_hold_and_coincident_points_separate() {
        let n = 200;
        let initial = vec![5.0; 2 * n];
        let mut l = ForceLayout::new(n as u32, vec![], vec![], vec![], Some(initial));
        l.pin(7, 100.0, -50.0);
        let f = l.step(100);
        assert_eq!((f.positions[14], f.positions[15]), (100.0, -50.0));
        assert!(f.positions.iter().all(|v| v.is_finite()));
        let spread = (0..n)
            .map(|i| dist(&f.positions, i, 0))
            .fold(0.0f32, f32::max);
        assert!(spread > 20.0, "repulsion pushes stacked nodes apart");
        l.unpin(7);
        l.reheat(0.5);
        assert!(l.step(1).alpha > 0.0);
    }

    #[test]
    fn handles_five_thousand_nodes_quickly() {
        let n = 5000u32;
        let (mut s, mut t) = (Vec::new(), Vec::new());
        for i in 0..n {
            for k in [1, 7, 31, 101] {
                s.push(i);
                t.push((i * 13 + k) % n);
            }
        }
        let w = vec![1.0; s.len()];
        let mut l = ForceLayout::new(n, s, t, w, None);
        let start = std::time::Instant::now();
        let f = l.step(10);
        assert!(f.positions.iter().all(|v| v.is_finite()));
        // Generous bound for debug builds on CI.
        assert!(start.elapsed().as_secs_f32() < 20.0);
    }
}
