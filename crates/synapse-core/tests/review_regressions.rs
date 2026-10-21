//! Regressions for bugs found in the integration review.

use synapse_core::{Asset, Graph, NewPage, Op, Placement};

fn graph() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    (dir, g)
}

fn sync(from: &Graph, to: &Graph, after: i64) {
    for r in from.ops_since(after, 100_000).unwrap() {
        to.apply_remote(&r.hlc, &r.op).unwrap();
    }
}

#[test]
fn concurrent_moves_creating_a_cycle_stay_visible_and_deletable() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    let p = a.create_page(NewPage::titled("P")).unwrap();
    let x = a
        .insert_block(p.id, Placement::LastChildOf(None), "x")
        .unwrap();
    let y = a
        .insert_block(p.id, Placement::LastChildOf(None), "y")
        .unwrap();
    sync(&a, &b, 0);
    let (sa, sb) = (a.last_seq().unwrap(), b.last_seq().unwrap());
    a.move_block(x.id, p.id, Placement::LastChildOf(Some(y.id)))
        .unwrap();
    b.move_block(y.id, p.id, Placement::LastChildOf(Some(x.id)))
        .unwrap();
    sync(&a, &b, sa);
    sync(&b, &a, sb);
    for g in [&a, &b] {
        assert_eq!(g.page_tree(p.id).unwrap().len(), 2);
        g.move_block(x.id, p.id, Placement::LastChildOf(Some(y.id)))
            .ok();
    }
    a.delete_block(x.id).unwrap();
    assert!(a.page_tree(p.id).unwrap().is_empty());
}

#[test]
fn three_replicas_with_the_same_title_converge() {
    let reps: Vec<_> = (0..3).map(|_| graph()).collect();
    for (_, g) in &reps {
        g.create_page(NewPage::titled("T")).unwrap();
    }
    for (i, (_, from)) in reps.iter().enumerate() {
        for (j, (_, to)) in reps.iter().enumerate() {
            if i != j {
                sync(from, to, 0);
            }
        }
    }
    // A second round propagates the renames done while resolving.
    for (i, (_, from)) in reps.iter().enumerate() {
        for (j, (_, to)) in reps.iter().enumerate() {
            if i != j {
                sync(from, to, 0);
            }
        }
    }
    let titles = |g: &Graph| {
        let mut t: Vec<_> = g
            .list_pages(false, 100, 0)
            .unwrap()
            .into_iter()
            .map(|p| (p.id, p.title))
            .collect();
        t.sort();
        t
    };
    let t0 = titles(&reps[0].1);
    assert_eq!(t0.len(), 3);
    assert_eq!(t0, titles(&reps[1].1));
    assert_eq!(t0, titles(&reps[2].1));
    assert_eq!(t0.iter().filter(|(_, t)| t == "T").count(), 1);
}

#[test]
fn alias_last_writer_wins() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    let pa = a.create_page(NewPage::titled("A")).unwrap();
    let pb = a.create_page(NewPage::titled("B")).unwrap();
    sync(&a, &b, 0);
    let (sa, sb) = (a.last_seq().unwrap(), b.last_seq().unwrap());
    a.set_alias("X", Some(pa.id)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    b.set_alias("X", Some(pb.id)).unwrap();
    sync(&a, &b, sa);
    sync(&b, &a, sb);
    for g in [&a, &b] {
        assert_eq!(g.page_by_title("x").unwrap().unwrap().id, pb.id);
    }
}

#[test]
fn incoming_title_does_not_rename_a_page_with_that_alias() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    let zed = a.create_page(NewPage::titled("Zed")).unwrap();
    a.set_alias("Alpha", Some(zed.id)).unwrap();
    b.create_page(NewPage::titled("Alpha")).unwrap();
    sync(&b, &a, 0);
    assert_eq!(a.get_page(zed.id).unwrap().unwrap().title, "Zed");
    assert_ne!(a.page_by_title("alpha").unwrap().unwrap().id, zed.id);
}

#[test]
fn placement_and_title_validation() {
    let (_d, g) = graph();
    let p = g.create_page(NewPage::titled("P")).unwrap();
    let q = g.create_page(NewPage::titled("Q")).unwrap();
    let on_q = g
        .insert_block(q.id, Placement::LastChildOf(None), "q")
        .unwrap();
    assert!(
        g.insert_block(p.id, Placement::LastChildOf(Some(on_q.id)), "bad")
            .is_err()
    );
    assert!(g.rename_page(p.id, "   ").is_err());
    assert!(g.rename_page(p.id, "2030-01-01").is_err());
    assert!(g.create_page(NewPage::titled("2030-01-01")).is_err());
    let j = g.ensure_page("2030-01-01", None).unwrap();
    assert!(j.is_journal());
}

#[test]
fn rename_rewrites_property_refs() {
    let (_d, g) = graph();
    let p = g.create_page(NewPage::titled("P")).unwrap();
    let b = g
        .insert_block(p.id, Placement::LastChildOf(None), "meeting")
        .unwrap();
    let mut props = synapse_core::Properties::new();
    props.insert("type".into(), "[[Old]]".into());
    g.set_block_properties(b.id, props).unwrap();
    let old = g.ensure_page("Old", None).unwrap();
    g.rename_page(old.id, "New").unwrap();
    let b = g.get_block(b.id).unwrap().unwrap();
    assert_eq!(b.properties["type"], "[[New]]");
}

#[test]
fn invalid_remote_asset_hash_is_ignored() {
    let (_d, g) = graph();
    let asset = Asset {
        hash: "ñ/../x".into(),
        size: 1,
        mime: None,
        original_name: None,
        created_at: 0,
    };
    assert!(
        !g.apply_remote("0000000000001-00000-zz", &Op::PutAsset { asset })
            .unwrap()
    );
    assert!(!g.blobs().contains("ñ/../x"));
}
