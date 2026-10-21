use chrono::{Local, NaiveDate, TimeZone};
use synapse_core::{Graph, NewPage, Op, Placement};

fn ms(y: i32, m: u32, d: u32) -> i64 {
    Local
        .with_ymd_and_hms(y, m, d, 12, 0, 0)
        .unwrap()
        .timestamp_millis()
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn graph() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    (dir, g)
}

#[test]
fn journals_chain_and_pages_are_anchored() {
    let (_d, g) = graph();
    g.set_now(Some(ms(2026, 10, 1)));
    let j1 = g.today_journal().unwrap();
    g.insert_block(
        j1.id,
        Placement::LastChildOf(None),
        "Idea about [[Rust FFI]]",
    )
    .unwrap();
    let page = g.page_by_title("rust ffi").unwrap().unwrap();
    assert_eq!(page.origin_journal, Some(day(2026, 10, 1)));

    g.set_now(Some(ms(2026, 10, 5)));
    g.journal(day(2026, 10, 5)).unwrap();
    g.insert_block(page.id, Placement::LastChildOf(None), "more notes")
        .unwrap();

    let n = g.journal_neighbors(day(2026, 10, 3)).unwrap();
    assert_eq!(
        (n.prev, n.next),
        (Some(day(2026, 10, 1)), Some(day(2026, 10, 5)))
    );
    assert_eq!(g.edit_days(page.id).unwrap(), vec![(day(2026, 10, 5), 1)]);

    let tl = g.timeline(day(2026, 9, 30), day(2026, 10, 6)).unwrap();
    assert_eq!(tl.len(), 2);
    assert_eq!(tl[0].created, vec![page.id]);
    assert_eq!(tl[1].edited, vec![(page.id, 1)]);
}

#[test]
fn outline_order_moves_and_deletes() {
    let (_d, g) = graph();
    let p = g.create_page(NewPage::titled("Outline")).unwrap();
    let a = g
        .insert_block(p.id, Placement::LastChildOf(None), "a")
        .unwrap();
    let c = g.insert_block(p.id, Placement::After(a.id), "c").unwrap();
    let b = g.insert_block(p.id, Placement::Before(c.id), "b").unwrap();
    let b1 = g
        .insert_block(p.id, Placement::LastChildOf(Some(b.id)), "b1")
        .unwrap();
    let order = |g: &Graph| {
        g.page_tree(p.id)
            .unwrap()
            .into_iter()
            .map(|(d, b)| format!("{d}{}", b.content))
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&g), ["0a", "0b", "1b1", "0c"]);
    assert!(
        g.move_block(b.id, p.id, Placement::LastChildOf(Some(b1.id)))
            .is_err()
    );
    g.move_block(c.id, p.id, Placement::Before(a.id)).unwrap();
    assert_eq!(order(&g), ["0c", "0a", "0b", "1b1"]);
    g.delete_block(b.id).unwrap();
    assert_eq!(order(&g), ["0c", "0a"]);
    assert!(g.get_block(b1.id).unwrap().is_none());
}

#[test]
fn backlinks_aliases_and_rename() {
    let (_d, g) = graph();
    let p = g.create_page(NewPage::titled("Notes")).unwrap();
    g.insert_block(
        p.id,
        Placement::LastChildOf(None),
        "about [[Brain]] and #brain",
    )
    .unwrap();
    let brain = g.page_by_title("Brain").unwrap().unwrap();
    g.set_alias("Cerebro", Some(brain.id)).unwrap();
    g.insert_block(p.id, Placement::LastChildOf(None), "en [[cerebro]]")
        .unwrap();
    assert_eq!(g.backlinks(brain.id).unwrap().len(), 3);
    assert_eq!(g.page_by_title("CEREBRO").unwrap().unwrap().id, brain.id);

    g.rename_page(brain.id, "Second Brain").unwrap();
    let contents: Vec<_> = g
        .page_tree(p.id)
        .unwrap()
        .into_iter()
        .map(|(_, b)| b.content)
        .collect();
    assert_eq!(contents[0], "about [[Second Brain]] and #[[Second Brain]]");
    assert_eq!(g.backlinks(brain.id).unwrap().len(), 3);
    assert!(g.synapse_weight(p.id, brain.id).unwrap() > 0.0);
}

#[test]
fn replicas_converge_through_op_log() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    let p = a.create_page(NewPage::titled("Shared")).unwrap();
    let blk = a
        .insert_block(p.id, Placement::LastChildOf(None), "v1")
        .unwrap();
    for r in a.ops_since(0, 1000).unwrap() {
        b.apply_remote(&r.hlc, &r.op).unwrap();
    }
    assert_eq!(b.get_block(blk.id).unwrap().unwrap().content, "v1");

    let seq_a = a.last_seq().unwrap();
    let seq_b = b.last_seq().unwrap();
    b.update_block(blk.id, "v2 from b").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    a.update_block(blk.id, "v3 from a").unwrap();
    let from_b = b.ops_since(seq_b, 1000).unwrap();
    let from_a = a.ops_since(seq_a, 1000).unwrap();
    for r in &from_b {
        a.apply_remote(&r.hlc, &r.op).unwrap();
    }
    for r in &from_a {
        b.apply_remote(&r.hlc, &r.op).unwrap();
    }
    let ca = a.get_block(blk.id).unwrap().unwrap().content;
    let cb = b.get_block(blk.id).unwrap().unwrap().content;
    assert_eq!(ca, cb);
    assert_eq!(ca, "v3 from a");

    let del = a.last_seq().unwrap();
    a.delete_block(blk.id).unwrap();
    for r in a.ops_since(del, 100).unwrap() {
        assert!(matches!(
            r.op,
            Op::DeleteBlock { .. } | Op::UpsertPage { .. }
        ));
        b.apply_remote(&r.hlc, &r.op).unwrap();
    }
    assert!(b.get_block(blk.id).unwrap().is_none());
}

#[test]
fn related_prefers_connected_notes() {
    let (_d, g) = graph();
    let hub = g.create_page(NewPage::titled("Hub")).unwrap();
    g.insert_block(
        hub.id,
        Placement::LastChildOf(None),
        "[[Near]] [[Also Near]]",
    )
    .unwrap();
    let near = g.page_by_title("Near").unwrap().unwrap();
    g.insert_block(near.id, Placement::LastChildOf(None), "[[Far]]")
        .unwrap();
    let related = g.related(hub.id, 10).unwrap();
    let pos = |t: &str| {
        let id = g.page_by_title(t).unwrap().unwrap().id;
        related.iter().position(|(p, _)| *p == id).unwrap()
    };
    assert!(pos("Near") < pos("Far"));
}

#[test]
fn assets_are_deduplicated() {
    let (_d, g) = graph();
    let a = g
        .put_asset_reader(&b"big file"[..], Some("x.pdf"), Some("application/pdf"))
        .unwrap();
    let b = g
        .put_asset_reader(&b"big file"[..], Some("y.pdf"), None)
        .unwrap();
    assert_eq!(a, b);
    assert!(g.blobs().contains(&a.hash));
}

#[test]
fn transactions_nest_and_roll_back() {
    let (_d, g) = graph();
    let r: synapse_core::Result<()> = g.transaction(|g| {
        g.create_page(NewPage::titled("Kept?"))?;
        Err(synapse_core::Error::Invalid("abort".into()))
    });
    assert!(r.is_err());
    assert!(g.page_by_title("Kept?").unwrap().is_none());
    g.transaction(|g| {
        let p = g.create_page(NewPage::titled("Outer"))?;
        g.insert_block(p.id, Placement::LastChildOf(None), "inner tx")?;
        Ok(())
    })
    .unwrap();
    assert!(g.page_by_title("Outer").unwrap().is_some());
}

#[test]
fn journals_before_pages_backwards() {
    let (_d, g) = graph();
    for d in [1, 3, 7] {
        g.journal(day(2026, 10, d)).unwrap();
    }
    let titles = |v: Vec<synapse_core::Page>| {
        v.into_iter()
            .map(|p| p.journal_date.unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        titles(g.journals_before(None, 2).unwrap()),
        vec![day(2026, 10, 7), day(2026, 10, 3)]
    );
    assert_eq!(
        titles(g.journals_before(Some(day(2026, 10, 3)), 5).unwrap()),
        vec![day(2026, 10, 1)]
    );
}
