use synapse_ffi::api::dto::{BlockDto, Place, PlaceKind};
use synapse_ffi::api::graph::SynapseGraph;

fn open() -> (tempfile::TempDir, SynapseGraph) {
    let dir = tempfile::tempdir().unwrap();
    let g = SynapseGraph::open(dir.path().to_string_lossy().into_owned()).unwrap();
    (dir, g)
}

fn place(kind: PlaceKind, anchor: Option<&str>) -> Place {
    Place {
        kind,
        anchor: anchor.map(str::to_string),
    }
}

/// `(depth, content)` rows for compact assertions.
fn shape(rows: &[BlockDto]) -> Vec<(u32, &str)> {
    rows.iter().map(|b| (b.depth, b.content.as_str())).collect()
}

fn page_with(g: &SynapseGraph, title: &str, lines: &[&str]) -> (String, Vec<String>) {
    let page = g.open_page(title.into()).unwrap().page.id;
    let mut ids = Vec::new();
    for l in lines {
        let b = g
            .insert_block(
                page.clone(),
                place(PlaceKind::LastChildOf, None),
                (*l).into(),
            )
            .unwrap();
        ids.push(b.id);
    }
    (page, ids)
}

#[test]
fn indent_and_outdent_keep_visual_order() {
    let (_d, g) = open();
    let (_, ids) = page_with(&g, "P", &["a", "b", "c", "d"]);
    g.indent_block(ids[1].clone()).unwrap();
    g.indent_block(ids[2].clone()).unwrap();
    let rows = g.indent_block(ids[3].clone()).unwrap();
    assert_eq!(shape(&rows), vec![(0, "a"), (1, "b"), (1, "c"), (1, "d")]);
    assert!(rows[0].has_children);

    // Outdenting `c` adopts `d` as its child (Logseq's direct outdenting).
    let rows = g.outdent_block(ids[2].clone()).unwrap();
    assert_eq!(shape(&rows), vec![(0, "a"), (1, "b"), (0, "c"), (1, "d")]);

    // A first child cannot be indented; a root block cannot be outdented.
    let rows = g.indent_block(ids[1].clone()).unwrap();
    assert_eq!(shape(&rows), vec![(0, "a"), (1, "b"), (0, "c"), (1, "d")]);
    let rows = g.outdent_block(ids[0].clone()).unwrap();
    assert_eq!(rows.len(), 4);
}

#[test]
fn merge_appends_text_and_adopts_children() {
    let (_d, g) = open();
    let (page, ids) = page_with(&g, "P", &["hello ", "world", "x"]);
    g.indent_block(ids[2].clone()).unwrap();
    let rows = g.merge_block(ids[1].clone(), ids[0].clone()).unwrap();
    assert_eq!(shape(&rows), vec![(0, "hello world"), (1, "x")]);
    assert_eq!(g.outline(page).unwrap().len(), 2);
}

#[test]
fn merging_a_first_child_into_its_parent_keeps_grandchildren_in_place() {
    let (_d, g) = open();
    let (_, ids) = page_with(&g, "P", &["p", "c", "gc", "s"]);
    g.indent_block(ids[1].clone()).unwrap();
    g.indent_block(ids[2].clone()).unwrap();
    g.indent_block(ids[2].clone()).unwrap();
    g.indent_block(ids[3].clone()).unwrap();
    // p > c > gc, p > s
    let rows = g.merge_block(ids[1].clone(), ids[0].clone()).unwrap();
    assert_eq!(shape(&rows), vec![(0, "pc"), (1, "gc"), (1, "s")]);
}

#[test]
fn insert_first_child_and_move() {
    let (_d, g) = open();
    let (page, ids) = page_with(&g, "P", &["a", "b"]);
    g.insert_block(
        page.clone(),
        place(PlaceKind::FirstChildOf, Some(&ids[0])),
        "a1".into(),
    )
    .unwrap();
    g.insert_block(
        page.clone(),
        place(PlaceKind::FirstChildOf, Some(&ids[0])),
        "a0".into(),
    )
    .unwrap();
    let rows = g
        .move_block(
            ids[1].clone(),
            page,
            place(PlaceKind::Before, Some(&ids[0])),
        )
        .unwrap();
    assert_eq!(shape(&rows), vec![(0, "b"), (0, "a"), (1, "a0"), (1, "a1")]);
}

#[test]
fn journals_feed_starts_today_and_walks_back() {
    let (_d, g) = open();
    for d in ["2024-01-01", "2024-01-03", "2024-02-10"] {
        g.journal(d.into()).unwrap();
    }
    let feed = g.journals(None, 3).unwrap();
    assert_eq!(
        feed[0].page.journal_date.as_deref(),
        Some(g.today().as_str())
    );
    assert_eq!(feed[1].page.title, "2024-02-10");
    assert_eq!(feed[2].page.title, "2024-01-03");
    assert_eq!(feed[2].prev_journal.as_deref(), Some("2024-01-01"));
    let more = g.journals(Some("2024-01-03".into()), 10).unwrap();
    assert_eq!(more.len(), 1);
    assert_eq!(more[0].page.title, "2024-01-01");
}

#[test]
fn links_backlinks_search_and_timeline() {
    let (_d, g) = open();
    let j = g.journal(g.today()).unwrap();
    let jb = g
        .insert_block(
            j.page.id.clone(),
            place(PlaceKind::LastChildOf, None),
            "met [[Rust Lang]] and #ideas".into(),
        )
        .unwrap();
    let rust = g.page_by_title("rust lang".into()).unwrap().unwrap();
    assert_eq!(
        rust.page.origin_journal.as_deref(),
        Some(g.today().as_str())
    );

    let groups = g.backlinks(rust.page.id.clone()).unwrap();
    assert_eq!(groups.len(), 1);
    assert!(groups[0].is_journal);
    assert_eq!(groups[0].blocks[0].id, jb.id);

    g.add_alias(rust.page.id.clone(), "Ferris".into()).unwrap();
    let hits = g.search_titles("fer".into(), 10).unwrap();
    assert_eq!(hits[0].page_id, rust.page.id);
    assert!(hits[0].is_alias);
    let hits = g.search_titles("lang".into(), 10).unwrap();
    assert_eq!(hits[0].title, "Rust Lang");

    let blocks = g.search_blocks("#ideas".into(), 10).unwrap();
    assert_eq!(blocks.len(), 1);
    assert!(g.search_blocks("100%".into(), 10).unwrap().is_empty());

    let r = g.block_ref(jb.id.clone()).unwrap().unwrap();
    assert_eq!(r.page_title, g.today());

    let t = g.timeline(g.today(), g.today()).unwrap();
    assert_eq!(t.days.len(), 1);
    assert!(t.days[0].created.contains(&rust.page.id));
    assert!(t.pages.iter().any(|p| p.title == "ideas"));

    g.record_visit(Some(j.page.id.clone()), rust.page.id.clone())
        .unwrap();
    let related = g.related(j.page.id, 5).unwrap();
    assert!(related.iter().any(|r| r.page.id == rust.page.id));
}

#[test]
fn assets_round_trip() {
    let (_d, g) = open();
    let a = g
        .put_asset_bytes(
            b"png".to_vec(),
            Some("x.png".into()),
            Some("image/png".into()),
        )
        .unwrap();
    let path = g.asset_path(a.hash.clone()).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"png");
    assert!(g.asset_path("zz".into()).is_none());
}
