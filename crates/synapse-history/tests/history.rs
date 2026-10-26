use std::time::Duration;
use synapse_core::{Graph, NewPage, Placement};
use synapse_history::{History, HistoryOptions, SnapshotPolicy, export};

const T0: i64 = 1_700_000_000_000;

fn setup() -> (tempfile::TempDir, Graph, History) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    g.set_now(Some(T0));
    let h = History::open(dir.path(), HistoryOptions::default()).unwrap();
    (dir, g, h)
}

fn add(g: &Graph, title: &str, blocks: &[&str]) -> synapse_core::Page {
    let p = g.ensure_page(title, None).unwrap();
    for b in blocks {
        g.insert_block(p.id, Placement::LastChildOf(None), b)
            .unwrap();
    }
    p
}

fn at(g: &Graph, ms: i64) {
    g.set_now(Some(ms));
}

#[test]
fn export_is_deterministic_and_outline_shaped() {
    let (_d, g, _h) = setup();
    let p = add(&g, "Project/Alpha", &["first", "second"]);
    let first = g.page_tree(p.id).unwrap()[0].1.clone();
    g.insert_block(p.id, Placement::LastChildOf(Some(first.id)), "child\nmore")
        .unwrap();
    let t = add(&g, "Target", &["target block"]);
    let tb = g.page_tree(t.id).unwrap()[0].1.id;
    g.insert_block(p.id, Placement::LastChildOf(None), &format!("see (({tb}))"))
        .unwrap();
    g.set_alias("Alpha", Some(p.id)).unwrap();
    let refs = export::referenced_blocks(g.conn()).unwrap();
    assert!(refs.contains(&tb));

    let a = export::render_page(&g, &p, &refs).unwrap();
    let b = export::render_page(&g, &p, &refs).unwrap();
    assert_eq!(a, b);
    assert!(
        a.starts_with("alias:: Alpha\n\n- first\n\t- child\n\t  more\n- second\n"),
        "{a}"
    );
    let tt = export::render_page(&g, &g.get_page(t.id).unwrap().unwrap(), &refs).unwrap();
    assert!(tt.contains(&format!("id:: {tb}")), "{tt}");
    assert_eq!(export::page_path(&p), "pages/Project___Alpha.md");
}

#[test]
fn incremental_snapshot_only_rewrites_touched_pages() {
    let (_d, g, mut h) = setup();
    add(&g, "A", &["a1"]);
    add(&g, "B", &["b1"]);
    let s1 = h.snapshot(&g, T0).unwrap().unwrap();
    assert!(s1.summary.contains("created"), "{}", s1.summary);
    assert!(
        h.snapshot(&g, T0 + 1000).unwrap().is_none(),
        "nothing changed"
    );

    at(&g, T0 + 10_000);
    let a = g.page_by_title("A").unwrap().unwrap();
    g.insert_block(a.id, Placement::LastChildOf(None), "a2")
        .unwrap();
    let s2 = h.snapshot(&g, T0 + 20_000).unwrap().unwrap();
    assert_eq!(s2.summary, "1 page edited: A");
    // B's blob is unchanged between snapshots.
    assert_eq!(
        h.page_at(&s1.id, "B").unwrap(),
        h.page_at(&s2.id, "B").unwrap()
    );
    assert_eq!(h.snapshots(10).unwrap().len(), 2);

    // Deleting a block (no page op) is still noticed.
    at(&g, T0 + 30_000);
    let b = g.page_by_title("B").unwrap().unwrap();
    let blk = g.page_tree(b.id).unwrap()[0].1.id;
    g.delete_block(blk).unwrap();
    let s3 = h.snapshot(&g, T0 + 40_000).unwrap().unwrap();
    assert_eq!(s3.summary, "1 page edited: B");
    assert_eq!(h.page_at(&s3.id, "B").unwrap().unwrap(), "");

    // Deleting and renaming pages updates the tree.
    at(&g, T0 + 50_000);
    g.rename_page(a.id, "A2").unwrap();
    g.delete_page(b.id).unwrap();
    let s4 = h.snapshot(&g, T0 + 60_000).unwrap().unwrap();
    assert!(s4.summary.contains("2 deleted"), "{}", s4.summary);
    assert!(h.page_at(&s4.id, "A").unwrap().is_none());
    assert!(h.page_at(&s4.id, "A2").unwrap().unwrap().contains("a2"));
}

#[test]
fn incremental_equals_full_export() {
    let (_d, g, mut h) = setup();
    let a = add(&g, "A", &["a1"]);
    let t = add(&g, "T", &["t1"]);
    h.snapshot(&g, T0).unwrap();
    at(&g, T0 + 1000);
    let tb = g.page_tree(t.id).unwrap()[0].1.id;
    g.insert_block(a.id, Placement::LastChildOf(None), &format!("(({tb}))"))
        .unwrap();
    let s = h.snapshot(&g, T0 + 2000).unwrap().unwrap();
    // Page T (untouched by ops) gained an `id::` because it is now referenced.
    assert!(
        h.page_at(&s.id, "T")
            .unwrap()
            .unwrap()
            .contains(&format!("id:: {tb}"))
    );

    // A fresh history over the same graph produces identical files.
    let dir2 = tempfile::tempdir().unwrap();
    let mut h2 = History::open_at(dir2.path(), HistoryOptions::default()).unwrap();
    let s2 = h2.snapshot(&g, T0 + 2000).unwrap().unwrap();
    for title in ["A", "T"] {
        assert_eq!(
            h.page_at("HEAD", title).unwrap(),
            h2.page_at(&s2.id, title).unwrap()
        );
    }
}

#[test]
fn policy_controls_maybe_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    g.set_now(Some(T0));
    let opts = HistoryOptions {
        policy: SnapshotPolicy {
            min_interval: Duration::from_secs(60),
            max_interval: Duration::from_secs(600),
            on_idle: Some(Duration::from_secs(30)),
        },
        include_blobs: false,
    };
    let mut h = History::open(dir.path(), opts).unwrap();
    add(&g, "A", &["x"]);
    let first = h.maybe_snapshot(&g, T0).unwrap();
    assert!(first.is_some(), "first snapshot is immediate");

    let a = g.page_by_title("A").unwrap().unwrap();
    g.insert_block(a.id, Placement::LastChildOf(None), "y")
        .unwrap();
    // Too soon (min_interval).
    assert!(h.maybe_snapshot(&g, T0 + 10_000).unwrap().is_none());
    // Past min_interval and idle for 60s (>= 30s) since the change was first observed.
    assert!(h.maybe_snapshot(&g, T0 + 70_000).unwrap().is_some());
    assert!(
        h.maybe_snapshot(&g, T0 + 80_000).unwrap().is_none(),
        "nothing pending"
    );

    // Continuous editing still snapshots at max_interval.
    for i in 0..30 {
        g.insert_block(a.id, Placement::LastChildOf(None), "z")
            .unwrap();
        let t = T0 + 120_000 + i * 20_000;
        let r = h.maybe_snapshot(&g, t).unwrap();
        if r.is_some() {
            assert!(t - (T0 + 70_000) >= 600_000);
            return;
        }
    }
    panic!("max_interval never fired");
}

#[test]
fn page_history_and_diff() {
    let (_d, g, mut h) = setup();
    let p = add(&g, "Notes", &["one"]);
    let s1 = h.snapshot(&g, T0).unwrap().unwrap();
    add(&g, "Other", &["noise"]);
    h.snapshot(&g, T0 + 1000).unwrap();
    at(&g, T0 + 5000);
    g.insert_block(p.id, Placement::LastChildOf(None), "two")
        .unwrap();
    let s3 = h.snapshot(&g, T0 + 6000).unwrap().unwrap();

    let versions = h.page_history("Notes").unwrap();
    assert_eq!(versions.len(), 2, "only snapshots that changed the page");
    assert_eq!(versions[0].snapshot.id, s3.id);
    assert_eq!(versions[1].snapshot.id, s1.id);
    let d = h.diff_page("Notes", &s1.id, &s3.id).unwrap();
    assert!(d.contains("+- two"), "{d}");
    assert!(!d.contains("noise"));
    // Abbreviated ids work.
    assert_eq!(h.page_at(&s1.id[..8], "Notes").unwrap().unwrap(), "- one\n");
}

#[test]
fn restore_page_and_graph_keep_block_ids_and_never_rewrite_history() {
    let (_d, g, mut h) = setup();
    let p = add(&g, "Doc", &["keep me", "will change"]);
    let tree = g.page_tree(p.id).unwrap();
    let (keep, change) = (tree[0].1.id, tree[1].1.id);
    let other = add(&g, "Gone", &["bye"]);
    let s1 = h.snapshot(&g, T0).unwrap().unwrap();
    let n_snaps = h.snapshots(100).unwrap().len();

    at(&g, T0 + 1000);
    g.update_block(change, "changed!").unwrap();
    g.insert_block(p.id, Placement::LastChildOf(Some(keep)), "extra child")
        .unwrap();
    g.delete_page(other.id).unwrap();
    add(&g, "New page", &["fresh"]);

    let r = h.restore_page(&g, "Doc", &s1.id, T0 + 2000).unwrap();
    assert_eq!(r.restored, 1);
    assert!(r.safety_snapshot.is_some());
    let texts: Vec<_> = g
        .page_tree(p.id)
        .unwrap()
        .into_iter()
        .map(|(d, b)| (d, b.content, b.id))
        .collect();
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0].1, "keep me");
    assert_eq!(texts[0].2, keep, "unchanged block keeps its id");
    assert_eq!(texts[1].1, "will change");
    // History only grew.
    assert!(h.snapshots(100).unwrap().len() > n_snaps);

    // Whole graph: deleted page returns, new page disappears.
    let r = h.restore_graph(&g, &s1.id, T0 + 3000).unwrap();
    assert!(r.removed >= 1);
    let gone = g.page_by_title("Gone").unwrap().expect("page restored");
    assert_eq!(g.page_tree(gone.id).unwrap()[0].1.content, "bye");
    assert!(g.page_by_title("New page").unwrap().is_none());

    // After restoring, the next snapshot has the same page content as s1.
    let s = h
        .snapshot(&g, T0 + 4000)
        .unwrap()
        .unwrap_or_else(|| h.snapshots(1).unwrap().remove(0));
    assert_eq!(
        h.page_at(&s.id, "Doc").unwrap(),
        h.page_at(&s1.id, "Doc").unwrap()
    );
    assert_eq!(
        h.page_at(&s.id, "Gone").unwrap(),
        h.page_at(&s1.id, "Gone").unwrap()
    );
}

#[test]
fn restore_preserves_referenced_block_ids_and_aliases() {
    let (_d, g, mut h) = setup();
    let t = add(&g, "Target", &["answer"]);
    let tb = g.page_tree(t.id).unwrap()[0].1.id;
    let r = add(&g, "Ref", &[&format!("(({tb}))")]);
    g.set_alias("Tgt", Some(t.id)).unwrap();
    g.set_page_properties(
        t.id,
        serde_json::from_str(r#"{"type":"note","n":3,"done":true,"s":"true"}"#).unwrap(),
    )
    .unwrap();
    let s1 = h.snapshot(&g, T0).unwrap().unwrap();
    at(&g, T0 + 10);
    g.delete_block(tb).unwrap();
    g.set_alias("Tgt", None).unwrap();
    g.set_page_properties(t.id, Default::default()).unwrap();
    h.restore_page(&g, "Target", &s1.id, T0 + 20).unwrap();
    assert_eq!(g.get_block(tb).unwrap().unwrap().content, "answer");
    assert_eq!(g.aliases_of(t.id).unwrap(), ["Tgt"]);
    let props = g.get_page(t.id).unwrap().unwrap().properties;
    assert_eq!(props["n"], 3);
    assert_eq!(props["done"], true);
    assert_eq!(props["s"], "true");
    assert_eq!(g.block_backrefs(tb).unwrap()[0].page_id, r.id);
}

#[test]
fn journals_and_assets() {
    let (dir, g, mut h) = setup();
    let day = chrono::NaiveDate::from_ymd_opt(2024, 3, 9).unwrap();
    let j = g.journal(day).unwrap();
    g.insert_block(j.id, Placement::LastChildOf(None), "met [[Someone]]")
        .unwrap();
    let src = dir.path().join("pic.png");
    std::fs::write(&src, b"not really a png").unwrap();
    let asset = g.put_asset_file(&src, Some("image/png")).unwrap();
    let s = h.snapshot(&g, T0).unwrap().unwrap();
    assert!(
        h.page_at(&s.id, "2024-03-09")
            .unwrap()
            .unwrap()
            .contains("met [[Someone]]")
    );
    let files = h.file(&s.id, "assets.json").unwrap().unwrap();
    assert!(files.contains(&asset.hash) && files.contains("image/png"));
    let _ = NewPage::default();
}

fn dump(g: &Graph) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for p in g.list_pages(true, 1000, 0).unwrap() {
        let blocks = g
            .page_tree(p.id)
            .unwrap()
            .into_iter()
            .map(|(d, b)| format!("{d}:{}:{}", b.id, b.content))
            .collect();
        out.push((p.title, blocks));
    }
    out.sort();
    out
}

#[test]
fn failed_restore_leaves_graph_unchanged() {
    let (_d, g, mut h) = setup();
    let a = add(&g, "Doc", &["one", "two", "three"]);
    add(&g, "Gone", &["bye", "bye2"]);
    let s1 = h.snapshot(&g, T0).unwrap().unwrap();
    at(&g, T0 + 1000);
    let b = g.page_tree(a.id).unwrap()[1].1.id;
    g.update_block(b, "edited").unwrap();
    g.delete_page(g.page_by_title("Gone").unwrap().unwrap().id)
        .unwrap();
    add(&g, "Fresh", &["new"]);
    h.snapshot(&g, T0 + 2000).unwrap();
    let before = dump(&g);
    let seq = g.last_seq().unwrap();

    for n in [0, 1, 3] {
        h.inject_restore_fault(Some(n));
        assert!(h.restore_graph(&g, &s1.id, T0 + 3000).is_err());
        assert_eq!(dump(&g), before, "graph restore, fault after {n}");
        assert_eq!(g.last_seq().unwrap(), seq, "no ops leaked");
        assert!(h.restore_page(&g, "Doc", &s1.id, T0 + 3000).is_err() || n == 3);
        assert_eq!(dump(&g), before, "page restore, fault after {n}");
    }
    h.inject_restore_fault(None);
    h.restore_graph(&g, &s1.id, T0 + 4000).unwrap();
    assert!(g.page_by_title("Gone").unwrap().is_some());
}

#[test]
fn tags_round_trip_through_snapshots() {
    let (_d, g, mut h) = setup();
    let note = add(&g, "Note", &["body"]);
    g.add_page_tag(note.id, "tecnología").unwrap();
    g.add_page_tag(note.id, "idea").unwrap();
    g.set_page_type(note.id, Some("concepto")).unwrap();
    let s1 = h.snapshot(&g, T0 + 500).unwrap().unwrap();
    assert!(h.page_at(&s1.id, "idea").unwrap().is_none());
    let md = h.page_at(&s1.id, "Note").unwrap().unwrap();
    assert!(
        md.contains(r#"synapse-tags:: ["idea","tecnología"]"#),
        "{md}"
    );
    assert!(md.contains("synapse-type:: concepto"), "{md}");

    at(&g, T0 + 1000);
    g.set_page_tags(note.id, vec![]).unwrap();
    g.set_page_type(note.id, None).unwrap();
    h.restore_graph(&g, &s1.id, T0 + 2000).unwrap();
    assert_eq!(g.get_page(note.id).unwrap().unwrap().types, ["concepto"]);
    assert!(g.page_by_title("idea").unwrap().is_none());
    assert_eq!(
        g.get_page(note.id).unwrap().unwrap().tags,
        ["idea", "tecnología"]
    );
    assert_eq!(g.pages_with_tag("idea").unwrap(), [note.id]);
}

#[test]
fn hubs_round_trip_kind_description_and_memberships() {
    let (_d, g, mut h) = setup();
    let top = g.create_hub("Top", "All my stuff,\nin one place").unwrap();
    let sub = g.create_hub("Sub", "").unwrap();
    let note = add(&g, "Note", &["body"]);
    g.add_to_hub(sub.id, top.id).unwrap();
    g.add_to_hub(note.id, sub.id).unwrap();
    let s1 = h.snapshot(&g, T0).unwrap().unwrap();
    let top_md = h.page_at(&s1.id, "Top").unwrap().unwrap();
    assert!(top_md.contains("synapse-kind:: hub"), "{top_md}");
    assert!(top_md.contains(r#"synapse-description:: "All my stuff,\nin one place""#));
    let note_md = h.page_at(&s1.id, "Note").unwrap().unwrap();
    assert!(note_md.contains(r#"synapse-hubs:: ["Sub"]"#), "{note_md}");

    // Renaming a hub re-exports its members (they list hubs by title).
    at(&g, T0 + 1000);
    g.rename_page(sub.id, "Sub 2").unwrap();
    let s2 = h.snapshot(&g, T0 + 1500).unwrap().unwrap();
    let note_md = h.page_at(&s2.id, "Note").unwrap().unwrap();
    assert!(note_md.contains(r#"synapse-hubs:: ["Sub 2"]"#), "{note_md}");

    // Break everything, then restore the graph.
    at(&g, T0 + 2000);
    g.convert_to_note(top.id).unwrap();
    g.remove_from_hub(note.id, sub.id).unwrap();
    g.delete_page(sub.id).unwrap();
    h.restore_graph(&g, &s1.id, T0 + 3000).unwrap();

    let top = g.page_by_title("Top").unwrap().unwrap();
    assert!(top.is_hub());
    assert_eq!(top.description, "All my stuff,\nin one place");
    assert!(
        g.page_tree(top.id).unwrap().is_empty(),
        "hubs have no blocks"
    );
    let sub = g.page_by_title("Sub").unwrap().expect("sub hub restored");
    assert!(g.page_by_title("Sub 2").unwrap().is_none());
    assert!(sub.is_hub());
    assert_eq!(g.hub_members(top.id).unwrap(), vec![sub.id]);
    assert_eq!(g.hub_members(sub.id).unwrap(), vec![note.id]);
    assert_eq!(g.hubs_of(note.id).unwrap(), vec![sub.id]);
    assert_eq!(g.page_tree(note.id).unwrap()[0].1.content, "body");

    // A single page restore brings back its memberships too.
    at(&g, T0 + 4000);
    g.set_page_hubs(note.id, vec![]).unwrap();
    h.restore_page(&g, "Note", &s1.id, T0 + 5000).unwrap();
    assert_eq!(g.hubs_of(note.id).unwrap(), vec![sub.id]);

    // Restoring a deleted hub alone brings its members back (they hold the membership).
    at(&g, T0 + 5100);
    g.delete_page(sub.id).unwrap();
    let s3 = h.snapshot(&g, T0 + 5200).unwrap().unwrap();
    let note_md = h.page_at(&s3.id, "Note").unwrap().unwrap();
    assert!(!note_md.contains("synapse-hubs"), "{note_md}");
    h.restore_page(&g, "Sub", &s1.id, T0 + 5300).unwrap();
    let sub = g.page_by_title("Sub").unwrap().unwrap();
    assert!(sub.is_hub());
    assert_eq!(g.hub_members(sub.id).unwrap(), vec![note.id]);
    assert_eq!(g.hubs_of(sub.id).unwrap(), vec![top.id]);

    // The next snapshot matches the original export.
    let s = h
        .snapshot(&g, T0 + 6000)
        .unwrap()
        .unwrap_or_else(|| h.snapshots(1).unwrap().remove(0));
    for title in ["Top", "Sub", "Note"] {
        assert_eq!(
            h.page_at(&s.id, title).unwrap(),
            h.page_at(&s1.id, title).unwrap(),
            "{title}"
        );
    }
}

#[test]
fn old_tag_kind_exports_parse_as_plain_notes() {
    let parsed = synapse_history::parse::parse_page("synapse-kind:: tag\n\n- body\n");
    assert!(!parsed.is_hub);
    assert!(!parsed.properties.contains_key("synapse-kind"));
}

#[test]
fn old_types_list_restores_its_first_entry() {
    let parsed =
        synapse_history::parse::parse_page("synapse-types:: [\"persona\",\"lugar\"]\n\n- x\n");
    assert_eq!(parsed.types, ["persona"]);
    let parsed = synapse_history::parse::parse_page("synapse-type:: lugar\n\n- x\n");
    assert_eq!(parsed.types, ["lugar"]);
}
