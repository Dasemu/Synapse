//! Generates synthetic blocks and prints index/query timings.
//! `cargo run --release -p synapse-search --example bench -- [blocks]`

use std::time::{Duration, Instant};
use synapse_core::{Graph, Placement};
use synapse_search::testing::HashEmbedder;
use synapse_search::{Filter, LinkMatcher, Search};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn pct(mut v: Vec<Duration>, p: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 * p) as usize).min(v.len() - 1)]
}

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("{label}: {:.2?}", t.elapsed());
    r
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(50_000);
    let dir = std::env::temp_dir().join(format!("synapse-bench-{}", std::process::id()));
    let g = Graph::open(&dir).unwrap();
    let syll = [
        "ca", "sa", "ción", "mar", "ten", "lo", "ver", "pro", "ject", "ñan", "dú", "re", "da",
        "tion", "ing", "sys",
    ];
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let vocab: Vec<String> = (0..8000)
        .map(|_| {
            (0..2 + rng.below(3))
                .map(|_| syll[rng.below(syll.len())])
                .collect::<String>()
                + &rng.below(40).to_string()
        })
        .collect();
    let pages: Vec<_> = (0..n / 100 + 1)
        .map(|i| {
            g.ensure_page(&format!("{} {}", vocab[rng.below(vocab.len())], i), None)
                .unwrap()
        })
        .collect();
    timed(&format!("generate {n} blocks (graph writes)"), || {
        for _ in 0..n {
            let words: Vec<&str> = (0..8 + rng.below(20))
                .map(|_| vocab[rng.below(vocab.len())].as_str())
                .collect();
            let p = &pages[rng.below(pages.len())];
            g.insert_block(p.id, Placement::LastChildOf(None), &words.join(" "))
                .unwrap();
        }
    });

    let mut s = Search::open(&g).unwrap();
    timed("full-text sync (index everything)", || s.sync(&g).unwrap());
    println!("index docs: {}", s.doc_count());

    let f = Filter::default();
    let queries: Vec<String> = (0..300)
        .map(|_| {
            let w = &vocab[rng.below(vocab.len())];
            w.chars().take(2 + rng.below(5)).collect::<String>()
        })
        .collect();
    let mut qs = Vec::new();
    for q in &queries {
        let t = Instant::now();
        s.quick_switch(&g, q, 10).unwrap();
        qs.push(t.elapsed());
    }
    println!(
        "quick_switch  p50 {:.2?}  p99 {:.2?}",
        pct(qs.clone(), 0.5),
        pct(qs, 0.99)
    );
    let mut ss = Vec::new();
    for q in &queries {
        let t = Instant::now();
        s.search(&g, q, &f, 20).unwrap();
        ss.push(t.elapsed());
    }
    println!(
        "search (prefix, 1 term)  p50 {:.2?}  p99 {:.2?}",
        pct(ss.clone(), 0.5),
        pct(ss, 0.99)
    );
    let mut ms = Vec::new();
    for _ in 0..200 {
        let q = format!(
            "{} {}",
            vocab[rng.below(vocab.len())],
            vocab[rng.below(vocab.len())]
        );
        let t = Instant::now();
        s.search(&g, &q, &f, 20).unwrap();
        ms.push(t.elapsed());
    }
    println!(
        "search (2 full terms)    p50 {:.2?}  p99 {:.2?}",
        pct(ms.clone(), 0.5),
        pct(ms, 0.99)
    );

    let emb = HashEmbedder::new(384);
    let stored = timed("embed + store all (fake embedder, 384 dims)", || {
        s.embed_pending(&g, &emb, 512, usize::MAX).unwrap()
    });
    println!("vectors stored: {stored}");
    let qv = emb.embed_one(&vocab[1]);
    let mut vs = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        s.vector_search(&g, "test/hash", &qv, &f, 20).unwrap();
        vs.push(t.elapsed());
    }
    println!("vector_search p50 {:.2?}", pct(vs, 0.5));
    let mut hs = Vec::new();
    for q in queries.iter().take(20) {
        let t = Instant::now();
        s.hybrid_search(&g, q, Some(&emb), &f, 20).unwrap();
        hs.push(t.elapsed());
    }
    println!("hybrid_search p50 {:.2?}", pct(hs, 0.5));

    let m = timed("LinkMatcher::new", || LinkMatcher::new(&g).unwrap());
    let t = Instant::now();
    let mut found = 0;
    for p in pages.iter().take(50) {
        found += m.suggest_page(&g, p.id).unwrap().len();
    }
    println!(
        "suggest_page x50 pages: {:.2?} ({found} suggestions)",
        t.elapsed()
    );
    timed("unlinked_references (1 page)", || {
        s.unlinked_references(&g, pages[0].id, 50).unwrap().len()
    });
    std::fs::remove_dir_all(dir).ok();
}
