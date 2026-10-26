//! `cargo run -p synapse-import --example import -- <logseq_dir> <graph_dir> [--skip-existing] [--keep-legacy]`

use std::path::PathBuf;
use synapse_core::Graph;
use synapse_import::{ExistingPages, ImportOptions, import_logseq};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skip = args.iter().any(|a| a == "--skip-existing");
    let keep_legacy = args.iter().any(|a| a == "--keep-legacy");
    let paths: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    let [src, dst] = paths.as_slice() else {
        eprintln!("usage: import <logseq_dir> <graph_dir> [--skip-existing] [--keep-legacy]");
        std::process::exit(2);
    };
    let graph = Graph::open(dst).expect("open graph");
    let opts = ImportOptions {
        existing: if skip {
            ExistingPages::Skip
        } else {
            ExistingPages::Update
        },
        skip_legacy_pages: !keep_legacy,
    };
    let started = std::time::Instant::now();
    let mut last = (None, 0);
    let report = import_logseq(src, &graph, &opts, |p| {
        let step = (p.done * 20).checked_div(p.total).unwrap_or(0);
        if last != (Some(p.phase), step) {
            eprintln!(
                "[{:.1?}] {:?}: {}/{}",
                started.elapsed(),
                p.phase,
                p.done,
                p.total
            );
            last = (Some(p.phase), step);
        }
    })
    .expect("import failed");
    println!(
        "pages {} | journals {} | blocks {} | assets {} ({} bytes) | updated {} | unchanged {} | {:.2?}",
        report.pages,
        report.journals,
        report.blocks,
        report.assets,
        report.bytes,
        report.updated,
        report.unchanged,
        started.elapsed()
    );
    for f in &report.org_files {
        println!("org (raw): {f}");
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
}
