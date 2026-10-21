//! Prints the page-level graph's size by edge kind and times extraction and layout.
//! `cargo run --release -p synapse-ffi --example graph_stats -- /path/to/graph`

use synapse_ffi::api::graph::SynapseGraph;
use synapse_ffi::api::layout::ForceLayout;

fn main() -> anyhow::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: graph_stats <graph dir>"))?;
    let g = SynapseGraph::open(dir)?;
    let t = std::time::Instant::now();
    let d = g.graph_data()?;
    let built = t.elapsed();
    let t = std::time::Instant::now();
    g.graph_data()?;
    let cached = t.elapsed();
    let mut by_kind = [0usize; 7];
    for &k in &d.kinds {
        by_kind[k as usize] += 1;
    }
    let journals = d.nodes.iter().filter(|n| n.journal_date.is_some()).count();
    let clusters = d.nodes.iter().map(|n| n.cluster).max().map_or(0, |c| c + 1);
    println!(
        "nodes {} ({journals} journals), edges {}, clusters {clusters}",
        d.nodes.len(),
        d.kinds.len()
    );
    let names = [
        "link",
        "tag",
        "property",
        "block_ref",
        "embed",
        "mention",
        "synapse",
    ];
    for (n, c) in names.iter().zip(by_kind) {
        println!("  {n:<10} {c}");
    }
    println!("build {built:?}, cached {cached:?}");
    let mut l = ForceLayout::new(
        d.nodes.len() as u32,
        d.sources.clone(),
        d.targets.clone(),
        d.weights.clone(),
        None,
    );
    let t = std::time::Instant::now();
    let mut ticks = 0;
    while l.step(10).alpha > 0.0 {
        ticks += 10;
    }
    println!("layout settled after ~{ticks} ticks in {:?}", t.elapsed());
    Ok(())
}
