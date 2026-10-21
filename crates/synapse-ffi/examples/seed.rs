//! Fills a graph with a few weeks of demo journals and pages, through the same API the app
//! uses. `cargo run -p synapse-ffi --example seed -- /path/to/graph`

use synapse_ffi::api::dto::{Place, PlaceKind};
use synapse_ffi::api::graph::SynapseGraph;

fn main() -> anyhow::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: seed <graph dir>"))?;
    let g = SynapseGraph::open(dir)?;
    let today = chrono::NaiveDate::parse_from_str(&g.today(), "%Y-%m-%d")?;
    let day = |back: i64| (today - chrono::Duration::days(back)).to_string();
    let last = |page: &str, content: &str| {
        g.insert_block(
            page.to_string(),
            Place {
                kind: PlaceKind::LastChildOf,
                anchor: None,
            },
            content.to_string(),
        )
    };
    let child = |page: &str, parent: &str, content: &str| {
        g.insert_block(
            page.to_string(),
            Place {
                kind: PlaceKind::LastChildOf,
                anchor: Some(parent.to_string()),
            },
            content.to_string(),
        )
    };

    let entries: &[(i64, &[&str])] = &[
        (
            24,
            &[
                "Started reading about [[Spreading Activation]] for the related panel",
                "TODO sketch the #timeline view",
            ],
        ),
        (
            21,
            &[
                "Call with Ana about [[Synapse]] sync design",
                "#rust ownership notes",
            ],
        ),
        (
            17,
            &[
                "DONE pick fonts: Inter + Noto Serif",
                "Idea: [[Hebbian Learning]] for links",
            ],
        ),
        (12, &["Weekly review", "Read [[Zettelkasten]] chapter 3"]),
        (
            8,
            &["Drafted [[Outliner Keymap]]", "TODO benchmark tantivy"],
        ),
        (5, &["[[Synapse]] FFI is green", "Lunch with Marta #people"]),
        (2, &["Thinking about [[Spreading Activation]] decay rates"]),
        (
            0,
            &[
                "## Today",
                "TODO review the **editor** key handling",
                "Linked [[Synapse]] and [[Zettelkasten]] ideas",
                "Code: `cargo test --workspace`",
            ],
        ),
    ];
    for (back, lines) in entries {
        let j = g.journal(day(*back))?;
        let pid = j.page.id.clone();
        if !j.blocks.is_empty() {
            continue;
        }
        let mut prev = None;
        for l in *lines {
            let b = last(&pid, l)?;
            prev.get_or_insert(b.id);
        }
        if *back == 0 {
            let first = prev.unwrap();
            child(&pid, &first, "Nested notes stay under their parent")?;
        }
    }

    let syn = g.open_page("Synapse".into())?;
    if syn.blocks.is_empty() {
        let id = syn.page.id;
        let a = last(&id, "A native, brain-like second brain")?;
        child(&id, &a.id, "Rust core + Flutter UI, no webviews")?;
        child(
            &id,
            &a.id,
            "Journals chained in time; pages born in journals",
        )?;
        last(
            &id,
            "Related: [[Hebbian Learning]], [[Spreading Activation]]",
        )?;
        last(&id, "DONE ship the outliner")?;
        g.add_alias(id, "Second Brain".into())?;
    }
    let sa = g.open_page("Spreading Activation".into())?;
    if sa.blocks.is_empty() {
        last(
            &sa.page.id,
            "Activate a note, propagate through weighted edges with decay",
        )?;
        last(&sa.page.id, "Used by the **Related** panel in [[Synapse]]")?;
    }
    println!("seeded {}", g.dir());
    Ok(())
}
