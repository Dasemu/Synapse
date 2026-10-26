use serde_json::json;
use synapse_core::{
    Error, Graph, HubConversion, NewPage, Op, PageId, PageKind, Placement, is_legacy_page,
    namespace_parent,
};

fn graph() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    (dir, g)
}

fn page(g: &Graph, title: &str, blocks: &[&str]) -> PageId {
    let p = g.ensure_page(title, None).unwrap();
    for b in blocks {
        g.insert_block(p.id, Placement::LastChildOf(None), b)
            .unwrap();
    }
    p.id
}

fn hub(g: &Graph, title: &str) -> PageId {
    g.create_hub(title, "").unwrap().id
}

fn titles(g: &Graph, ids: &[PageId]) -> Vec<String> {
    ids.iter()
        .map(|&i| g.get_page(i).unwrap().unwrap().title)
        .collect()
}

fn texts(g: &Graph, id: PageId) -> Vec<(usize, String)> {
    g.page_tree(id)
        .unwrap()
        .into_iter()
        .map(|(d, b)| (d, b.content))
        .collect()
}

fn invalid<T: std::fmt::Debug>(r: synapse_core::Result<T>) -> bool {
    matches!(r, Err(Error::Invalid(_)))
}

/// Ships every op of `from` newer than `after` to `to`; returns how many applied.
fn ship(from: &Graph, to: &Graph, after: i64) -> usize {
    from.ops_since(after, 100_000)
        .unwrap()
        .iter()
        .filter(|r| to.apply_remote(&r.hlc, &r.op).unwrap())
        .count()
}

#[test]
fn a_page_belongs_to_at_most_one_hub() {
    let (_d, g) = graph();
    let media = g.create_hub("Media", "  Things I watch  ").unwrap();
    assert_eq!(media.kind, PageKind::Hub);
    assert_eq!(media.description, "Things I watch");
    let work = hub(&g, "Work");
    let books = hub(&g, "Books");
    let zeta = page(&g, "Zeta", &["z"]);
    let alpha = page(&g, "Alpha", &["a"]);

    g.add_to_hub(zeta, media.id).unwrap();
    g.add_to_hub(alpha, media.id).unwrap();
    g.add_to_hub(books, media.id).unwrap();
    // Duplicates are no-ops.
    let seq = g.last_seq().unwrap();
    g.add_to_hub(alpha, media.id).unwrap();
    assert_eq!(g.last_seq().unwrap(), seq);

    // Sub-hubs first, then notes, each by title.
    assert_eq!(
        titles(&g, &g.hub_members(media.id).unwrap()),
        ["Books", "Alpha", "Zeta"]
    );
    assert_eq!(titles(&g, &g.hubs_of(alpha).unwrap()), ["Media"]);

    // A page lives in one hub: adding it elsewhere moves it.
    g.add_to_hub(alpha, work).unwrap();
    assert_eq!(titles(&g, &g.hubs_of(alpha).unwrap()), ["Work"]);
    assert_eq!(g.get_page(alpha).unwrap().unwrap().hubs, [work]);
    assert_eq!(
        titles(&g, &g.hub_members(media.id).unwrap()),
        ["Books", "Zeta"]
    );
    assert!(invalid(g.set_page_hubs(zeta, vec![work, media.id])));
    let p = g.set_page_hubs(zeta, vec![work, work]).unwrap();
    assert_eq!(p.hubs, [work]);
    g.remove_from_hub(zeta, work).unwrap();
    assert!(g.hubs_of(zeta).unwrap().is_empty());
    // A template cannot leave its hub.
    let t = g.hub_template(work).unwrap();
    assert!(invalid(g.add_to_hub(t, media.id)));

    let s = g.set_hub_description(media.id, "Screen and paper").unwrap();
    assert_eq!(s.description, "Screen and paper");
    assert!(invalid(g.set_hub_description(zeta, "x")));
}

#[test]
fn hubs_have_no_blocks() {
    let (_d, g) = graph();
    let h = hub(&g, "Hub");
    let n = page(&g, "Note", &["one"]);
    assert!(invalid(g.insert_block(
        h,
        Placement::LastChildOf(None),
        "x"
    )));
    let b = g.page_tree(n).unwrap()[0].1.clone();
    assert!(invalid(g.move_block(b.id, h, Placement::LastChildOf(None))));
    let mut moved = b.clone();
    moved.page_id = h;
    assert!(invalid(g.put_block(&moved)));
    assert!(g.page_tree(h).unwrap().is_empty());
    // A date title cannot be a hub.
    assert!(invalid(g.create_hub("2024-01-02", "")));
}

#[test]
fn membership_validation_and_cycles() {
    let (_d, g) = graph();
    let a = hub(&g, "A");
    let b = hub(&g, "B");
    let c = hub(&g, "C");
    let note = page(&g, "Note", &[]);
    let j = g
        .journal(chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap())
        .unwrap();
    g.add_to_hub(b, a).unwrap(); // A ⊃ B
    g.add_to_hub(c, b).unwrap(); // B ⊃ C

    assert!(invalid(g.add_to_hub(a, a)), "self");
    assert!(invalid(g.add_to_hub(a, c)), "A inside C would be a cycle");
    assert!(invalid(g.add_to_hub(a, b)), "direct cycle");
    assert!(invalid(g.add_to_hub(j.id, a)), "journals are never members");
    assert!(invalid(g.add_to_hub(note, note)));
    assert!(invalid(g.add_to_hub(a, note)), "a note is not a hub");
    assert!(matches!(
        g.add_to_hub(note, synapse_core::new_id()),
        Err(Error::NotFound(_))
    ));
    // A note can be in a hub nested anywhere; a hub can be in two branches (DAG).
    g.add_to_hub(note, c).unwrap();
    g.add_to_hub(c, a).unwrap();
    let s = g.structure().unwrap();
    let info = |id| s.hubs.iter().find(|h| h.page == id).unwrap();
    assert_eq!(info(a).members, [b, c]);
    assert_eq!(info(c).depth, 1, "shortest path from a root");
    assert_eq!(s.home[&note], c);
}

#[test]
fn delete_page_cleans_memberships() {
    let (_d, g) = graph();
    let h = hub(&g, "H");
    let other = hub(&g, "Other");
    let n = page(&g, "N", &["x"]);
    g.add_to_hub(n, h).unwrap();
    g.add_to_hub(h, other).unwrap();

    let seq = g.last_seq().unwrap();
    g.delete_page(h).unwrap();
    assert!(g.get_page(n).unwrap().unwrap().hubs.is_empty());
    let logged: Vec<_> = g
        .ops_since(seq, 100)
        .unwrap()
        .into_iter()
        .filter_map(|r| match r.op {
            Op::UpsertPage { page } => Some(page),
            _ => None,
        })
        .collect();
    assert!(logged.iter().any(|p| p.id == n && p.hubs.is_empty()));
    assert!(g.hub_members(other).unwrap().is_empty());

    g.delete_page(n).unwrap();
    let s = g.structure().unwrap();
    assert_eq!(s.hubs.len(), 1);
    assert!(s.hubs[0].members.is_empty());
}

#[test]
fn conversion_preview() {
    let (_d, g) = graph();
    g.set_now(Some(1_700_000_000_000));
    let p = page(
        &g,
        "Entretenimiento/TV/Anime",
        &[
            "## Series",
            "[[Naruto]]",
            "#Bleach, [[2024-01-02]]",
            "type:: [[Genre]]",
            "My **favourite** ones, see [[One Piece]] and #[[Death Note]]",
            "[[Entretenimiento/TV/Anime]]",
            "[[Naruto]] again with words",
        ],
    );
    // A dangling alias (its page is gone) must not break anything.
    let gone = page(&g, "Gone", &[]);
    g.set_alias("Ghost", Some(gone)).unwrap();
    g.conn()
        .execute("DELETE FROM pages WHERE id=?1", [gone.to_string()])
        .unwrap();
    page(&g, "Uses", &["mentions [[Ghost]]"]);

    let prop = page(&g, "Props", &[]);
    let mut props = serde_json::Map::new();
    props.insert("parent".into(), json!("[[Entretenimiento/TV/Anime]]"));
    g.set_page_properties(prop, props).unwrap();
    g.set_alias("Animu", Some(p)).unwrap();
    page(&g, "Tagger", &["watch #Animu"]);
    page(
        &g,
        "BlockProp",
        &["x\ncategory:: [[Entretenimiento/TV/Anime]]"],
    );
    let j = g
        .journal(chrono::NaiveDate::from_ymd_opt(2024, 3, 3).unwrap())
        .unwrap();
    g.insert_block(j.id, Placement::LastChildOf(None), "#Animu")
        .unwrap();

    let pv = g.hub_conversion_preview(p).unwrap();
    assert_eq!(pv.suggested_title, "Anime");
    assert_eq!(
        pv.suggested_description,
        "My favourite ones, see One Piece and Death Note"
    );
    assert_eq!(
        titles(&g, &pv.members),
        [
            "Naruto",
            "Bleach",
            "One Piece",
            "Death Note",
            "Props",
            "BlockProp",
            "Tagger"
        ]
    );
    assert_eq!(pv.prose_blocks, 2);
    assert_eq!(pv.blocks, 7);

    let plain = page(&g, "Plain", &[]);
    let pv = g.hub_conversion_preview(plain).unwrap();
    assert_eq!(pv.suggested_title, "Plain");
    assert!(pv.suggested_description.is_empty() && pv.members.is_empty());
    assert!(invalid(g.hub_conversion_preview(j.id)));
    let h = hub(&g, "Hub");
    assert!(invalid(g.hub_conversion_preview(h)));
}

#[test]
fn convert_to_hub_shortens_title_and_keeps_outline() {
    let (_d, g) = graph();
    let p = page(&g, "Media/Anime", &["Intro text", "[[Naruto]]"]);
    let root = g.page_tree(p).unwrap()[0].1.id;
    g.insert_block(p, Placement::LastChildOf(Some(root)), "child")
        .unwrap();
    let parent = hub(&g, "Media");
    g.add_to_hub(p, parent).unwrap();
    let refr = page(&g, "Elsewhere", &["see [[Media/Anime]]"]);
    let naruto = g.page_by_title("Naruto").unwrap().unwrap().id;
    let j = g
        .journal(chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap())
        .unwrap();

    let h = g
        .convert_to_hub(
            p,
            &HubConversion {
                title: "Anime".into(),
                description: " Japanese animation ".into(),
                // Journals and cycles (the hub's own parent) are skipped silently.
                members: vec![naruto, j.id, parent, naruto],
                keep_outline_as: Some("Anime (notes)".into()),
            },
        )
        .unwrap();
    assert_eq!(h.title, "Anime");
    assert!(h.is_hub() && h.description == "Japanese animation");
    assert_eq!(h.hubs, [parent], "the hub stays where it was");
    assert!(g.page_tree(p).unwrap().is_empty());
    assert_eq!(texts(&g, refr), [(0, "see [[Anime]]".to_string())]);
    let note = g.page_by_title("Anime (notes)").unwrap().unwrap();
    assert_eq!(
        texts(&g, note.id),
        [
            (0, "Intro text".to_string()),
            (1, "child".to_string()),
            (0, "[[Naruto]]".to_string())
        ]
    );
    assert_eq!(
        titles(&g, &g.hub_members(p).unwrap()),
        ["Anime (notes)", "Naruto"]
    );
    assert!(g.get_page(parent).unwrap().unwrap().hubs.is_empty());

    // Back to a note: description, then one link per member; members leave the hub.
    let n = g.convert_to_note(p).unwrap();
    assert_eq!(n.kind, PageKind::Note);
    assert!(n.description.is_empty());
    assert_eq!(
        texts(&g, p),
        [
            (0, "Japanese animation".to_string()),
            (0, "[[Anime (notes)]]".to_string()),
            (0, "[[Naruto]]".to_string())
        ]
    );
    assert!(g.hub_members(p).unwrap().is_empty());
    assert!(g.get_page(naruto).unwrap().unwrap().hubs.is_empty());
    assert!(invalid(g.convert_to_note(p)));
}

#[test]
fn convert_to_hub_without_outline_and_title_clash() {
    let (_d, g) = graph();
    let p = page(&g, "Projects/Ideas", &["[[A]]", "some prose"]);
    page(&g, "Ideas", &["taken"]);
    let before = g.last_seq().unwrap();
    let clash = g.convert_to_hub(
        p,
        &HubConversion {
            title: "Ideas".into(),
            ..Default::default()
        },
    );
    assert!(matches!(clash, Err(Error::Conflict(_))), "{clash:?}");
    // Nothing changed.
    assert_eq!(g.last_seq().unwrap(), before);
    let cur = g.get_page(p).unwrap().unwrap();
    assert_eq!(cur.title, "Projects/Ideas");
    assert!(!cur.is_hub());
    assert_eq!(g.page_tree(p).unwrap().len(), 2);

    // An existing title for the kept-outline note is refused too.
    let clash = g.convert_to_hub(
        p,
        &HubConversion {
            title: "Projects/Ideas".into(),
            keep_outline_as: Some("ideas".into()),
            ..Default::default()
        },
    );
    assert!(matches!(clash, Err(Error::Conflict(_))));
    assert_eq!(g.page_tree(p).unwrap().len(), 2);

    let a = g.page_by_title("A").unwrap().unwrap().id;
    let h = g
        .convert_to_hub(
            p,
            &HubConversion {
                title: "Projects/Ideas".into(),
                description: String::new(),
                members: vec![a],
                keep_outline_as: None,
            },
        )
        .unwrap();
    assert_eq!(h.title, "Projects/Ideas");
    assert!(g.page_tree(p).unwrap().is_empty());
    assert_eq!(g.hub_members(p).unwrap(), [a]);
    assert!(invalid(g.convert_to_hub(p, &HubConversion::default())));
}

#[test]
fn structure_depth_home_and_dangling_ids() {
    let (_d, g) = graph();
    let root = hub(&g, "Root");
    let sub = hub(&g, "Sub");
    let leaf = page(&g, "Leaf", &[]);
    let r1 = page(&g, "R1", &[]);
    g.add_to_hub(sub, root).unwrap();
    g.add_to_hub(leaf, sub).unwrap();
    g.add_to_hub(r1, root).unwrap();
    // Dangling ids: a deleted page and a note.
    let gone = synapse_core::new_id();
    let mut p = g.get_page(r1).unwrap().unwrap();
    p.hubs.push(gone);
    p.hubs.push(leaf);
    g.put_page(&p).unwrap();

    let s = g.structure().unwrap();
    let info = |id| s.hubs.iter().find(|h| h.page == id).unwrap();
    assert_eq!((info(root).depth, info(sub).depth), (0, 1));
    assert_eq!(info(root).members, [sub, r1]);
    assert_eq!(info(sub).members, [leaf]);
    assert_eq!(s.home[&leaf], sub, "deepest wins");
    assert_eq!(
        (s.home[&sub], s.home[&root], s.home[&r1]),
        (sub, root, root)
    );
    assert_eq!(s.hubs.len(), 2);
    assert_eq!(g.hubs_of(r1).unwrap(), [root]);
    let alone = page(&g, "Alone", &[]);
    assert!(!g.structure().unwrap().home.contains_key(&alone));
}

#[test]
fn replicas_converge_in_any_order_and_break_concurrent_cycles() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    let h = a.create_hub("Hub", "desc").unwrap().id;
    let x = hub(&a, "X");
    let n = page(&a, "Note", &["text"]);
    a.add_to_hub(n, h).unwrap();
    a.add_to_hub(x, h).unwrap();

    // Out of order: members' pages arrive before the hub page.
    let mut ops = a.ops_since(0, 1000).unwrap();
    ops.reverse();
    for r in &ops {
        b.apply_remote(&r.hlc, &r.op).unwrap();
    }
    let hub_b = b.get_page(h).unwrap().unwrap();
    assert!(hub_b.is_hub() && hub_b.description == "desc");
    assert_eq!(b.structure().unwrap(), a.structure().unwrap());
    assert_eq!(b.hub_members(h).unwrap(), [x, n]);
    // Idempotent: replaying applies nothing; applied ops were re-logged on B.
    assert_eq!(ship(&a, &b, 0), 0);
    assert!(
        b.ops_since(0, 1000)
            .unwrap()
            .iter()
            .any(|r| matches!(&r.op, Op::UpsertPage { page } if page.id == n && page.hubs == [h]))
    );
    // Hub deletion on B reaches A, members included.
    let seq_b = b.last_seq().unwrap();
    b.delete_page(x).unwrap();
    ship(&b, &a, seq_b);
    assert!(a.get_page(x).unwrap().is_none());
    assert_eq!(a.hub_members(h).unwrap(), [n]);

    // Concurrent cycle: A puts Y in Z while B puts Z in Y.
    let y = hub(&a, "Y");
    let z = hub(&a, "Z");
    ship(&a, &b, 0);
    let (sa, sb) = (a.last_seq().unwrap(), b.last_seq().unwrap());
    a.add_to_hub(y, z).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    b.add_to_hub(z, y).unwrap();
    ship(&a, &b, sa);
    ship(&b, &a, sb);
    let (s1, s2) = (a.structure().unwrap(), b.structure().unwrap());
    assert_eq!(s1, s2);
    let members = |s: &synapse_core::Structure, id| {
        s.hubs
            .iter()
            .find(|h| h.page == id)
            .unwrap()
            .members
            .clone()
    };
    // The older write (Y in Z) stands; the one closing the cycle is ignored.
    assert_eq!(members(&s1, z), [y]);
    assert!(members(&s1, y).is_empty());
}

#[test]
fn old_ops_and_v2_databases_load() {
    // An op written before page kinds existed.
    let id = synapse_core::new_id();
    let op: Op = serde_json::from_value(json!({
        "type": "upsert_page",
        "page": {"id": id, "title": "Old", "journal_date": null, "origin_journal": null,
                 "properties": {}, "created_at": 1, "updated_at": 2}
    }))
    .unwrap();
    let Op::UpsertPage { page } = op else {
        panic!()
    };
    assert_eq!(page.kind, PageKind::Note);
    assert!(page.description.is_empty() && page.hubs.is_empty());

    // A database created with the v2 schema.
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = rusqlite::Connection::open(dir.path().join("synapse.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('schema', '2'), ('actor', 'oldactor0001');
             CREATE TABLE pages (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, title_key TEXT NOT NULL UNIQUE,
                journal_date TEXT UNIQUE, origin_journal TEXT,
                properties TEXT NOT NULL DEFAULT '{}', created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL, hlc TEXT NOT NULL);
             CREATE TABLE aliases (alias_key TEXT PRIMARY KEY, alias TEXT NOT NULL,
                page_id TEXT NOT NULL, hlc TEXT NOT NULL DEFAULT '');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pages VALUES (?1, 'Legacy', 'legacy', NULL, NULL, '{}', 1, 2, '0000000000001-00000-oldactor0001')",
            [id.to_string()],
        )
        .unwrap();
    }
    let g = Graph::open(dir.path()).unwrap();
    let schema: String = g
        .conn()
        .query_row("SELECT value FROM meta WHERE key='schema'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(schema, "7");
    let p = g.get_page(id).unwrap().unwrap();
    assert_eq!((p.kind, p.title.as_str()), (PageKind::Note, "Legacy"));
    let h = g.create_hub("New hub", "").unwrap();
    g.add_to_hub(id, h.id).unwrap();
    assert_eq!(g.hub_members(h.id).unwrap(), [id]);
    g.add_page_tag(id, "idea").unwrap();
    assert_eq!(g.pages_with_tag("idea").unwrap(), [id]);
    drop(g);
    // Reopening keeps everything (migration is idempotent).
    let g = Graph::open(dir.path()).unwrap();
    assert_eq!(g.hub_members(h.id).unwrap(), [id]);
    let n = g.create_page(NewPage::titled("After")).unwrap();
    assert_eq!(n.kind, PageKind::Note);
}

#[test]
fn namespace_helpers() {
    assert_eq!(namespace_parent("A/B/C"), Some("A/B"));
    assert_eq!(namespace_parent("A / B"), Some("A"));
    assert_eq!(namespace_parent("A"), None);
    assert_eq!(namespace_parent("/A"), None);
    assert_eq!(namespace_parent("A/"), None);
    assert_eq!(namespace_parent("A//B"), None);

    let (_d, g) = graph();
    let ab = page(&g, "A/B", &[]);
    let abc = page(&g, "A/B/C", &[]);
    let ad = page(&g, "A/d", &[]);
    page(&g, "X/Y/Z", &[]);
    let kids = g.namespace_children("a").unwrap();
    assert_eq!(
        kids,
        vec![("A/B".to_string(), Some(ab)), ("A/d".to_string(), Some(ad))]
    );
    assert_eq!(
        g.namespace_children("A/B").unwrap(),
        vec![("A/B/C".to_string(), Some(abc))]
    );
    // Virtual intermediate levels.
    assert_eq!(
        g.namespace_children("X").unwrap(),
        vec![("X/Y".to_string(), None)]
    );
    assert_eq!(g.namespace_children("X/Y").unwrap().len(), 1);
    assert!(g.namespace_children("nothing").unwrap().is_empty());
}

#[test]
fn legacy_titles() {
    for t in [
        "contents",
        "TODO",
        "All pages",
        "hls__book_123",
        "In-Progress",
    ] {
        assert!(is_legacy_page(t), "{t}");
    }
    for t in ["Todo list", "Projects", "hls"] {
        assert!(!is_legacy_page(t), "{t}");
    }
}

#[test]
fn every_hub_has_a_hidden_template_that_seeds_new_notes() {
    let (_d, g) = graph();
    let h = hub(&g, "Fun");
    let t = g.hub_template(h).unwrap();
    assert_eq!(g.get_page(t).unwrap().unwrap().kind, PageKind::Template);
    assert_eq!(g.hub_template(h).unwrap(), t, "idempotent");
    // Hidden from members, listings and the structure.
    assert!(g.hub_members(h).unwrap().is_empty());
    assert!(
        g.list_pages(true, 100, 0)
            .unwrap()
            .iter()
            .all(|p| p.id != t)
    );
    assert!(g.structure().unwrap().hubs[0].members.is_empty());

    let root = g
        .insert_block(t, Placement::LastChildOf(None), "Rating")
        .unwrap();
    g.insert_block(t, Placement::LastChildOf(Some(root.id)), "Notes")
        .unwrap();
    let n = g.create_page_from_template(h, "Chess").unwrap();
    assert_eq!(g.hubs_of(n.id).unwrap(), vec![h]);
    assert_eq!(
        texts(&g, n.id),
        vec![(0, "Rating".to_string()), (1, "Notes".to_string())]
    );

    // An empty template is fine, and the template cannot be deleted alone.
    let h2 = hub(&g, "Empty");
    let n2 = g.create_page_from_template(h2, "Blank").unwrap();
    assert!(texts(&g, n2.id).is_empty());
    assert!(invalid(g.delete_page(t)));
    g.delete_page(h).unwrap();
    assert!(g.get_page(t).unwrap().is_none());
}

#[test]
fn converting_to_a_hub_creates_a_template_and_back_removes_it() {
    let (_d, g) = graph();
    let p = page(&g, "Projects", &["a"]);
    g.convert_to_hub(
        p,
        &HubConversion {
            title: "Projects".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let t = g.hub_template(p).unwrap();
    g.convert_to_note(p).unwrap();
    assert!(g.get_page(t).unwrap().is_none());
}

#[test]
fn tags_are_names_many_per_page_kept_sorted() {
    let (_d, g) = graph();
    let a = page(&g, "Rust", &["x"]);
    let b = page(&g, "Madrid", &["y"]);
    let pages_before = g.list_pages(true, 100, 0).unwrap().len();

    g.add_page_tag(a, "  #Tecnología ").unwrap();
    g.add_page_tag(a, "herramientas").unwrap();
    g.add_page_tag(a, "TECNOLOGÍA").unwrap(); // same tag, other case
    g.add_page_tag(a, "Álgebra").unwrap();
    g.add_page_tag(b, "Tecnología").unwrap();
    let tags = g.get_page(a).unwrap().unwrap().tags;
    assert_eq!(tags, ["herramientas", "Tecnología", "Álgebra"]);
    // Tags never become pages.
    assert_eq!(g.list_pages(true, 100, 0).unwrap().len(), pages_before);
    assert!(g.page_by_title("tecnología").unwrap().is_none());

    assert_eq!(
        g.pages_with_tag("tecnologia").unwrap().len(),
        0,
        "accents matter"
    );
    let mut with = g.pages_with_tag("Tecnología").unwrap();
    with.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(with, want);

    let cat = g.tag_catalog().unwrap();
    assert_eq!((cat[0].name.as_str(), cat[0].count), ("Tecnología", 2));
    assert_eq!(cat.len(), 3, "no seeded tags");

    g.remove_page_tag(a, "TECNOLOGÍA").unwrap();
    assert_eq!(g.pages_with_tag("Tecnología").unwrap(), [b]);
    g.set_page_tags(a, vec![]).unwrap();
    assert!(g.get_page(a).unwrap().unwrap().tags.is_empty());

    // Rules. A page may carry a tag equal to its own title.
    let j = g
        .journal(chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap())
        .unwrap();
    assert!(invalid(g.add_page_tag(j.id, "x")));
    let h = hub(&g, "H");
    let t = g.hub_template(h).unwrap();
    assert!(invalid(g.add_page_tag(t, "x")));
    assert!(invalid(g.add_page_tag(a, &"x".repeat(49))));
    g.add_page_tag(a, "Rust").unwrap();
    g.remove_page_tag(a, "Rust").unwrap();
    g.add_page_tag(a, "   ").unwrap(); // blank: ignored
    assert!(g.get_page(a).unwrap().unwrap().tags.is_empty());

    g.delete_page(b).unwrap();
    assert!(g.pages_with_tag("Tecnología").unwrap().is_empty());
}

#[test]
fn a_page_with_a_tag_title_is_untouched_by_tagging() {
    let (_d, g) = graph();
    let n = page(&g, "Neurociencia", &["brains", "memory"]);
    let other = page(&g, "Plasticidad", &["x"]);
    g.add_page_tag(other, "neurociencia").unwrap();
    assert_eq!(g.get_page(n).unwrap().unwrap().kind, PageKind::Note);
    assert_eq!(texts(&g, n).len(), 2);
    assert_eq!(g.get_page(other).unwrap().unwrap().tags, ["neurociencia"]);
    g.rename_page(n, "Cerebro").unwrap();
    assert_eq!(g.get_page(other).unwrap().unwrap().tags, ["neurociencia"]);
}

#[test]
fn a_page_has_one_type_that_implies_a_tag() {
    let (_d, g) = graph();
    let a = page(&g, "Ana", &["x"]);
    let b = page(&g, "Madrid", &["y"]);
    let pages_before = g.list_pages(true, 100, 0).unwrap().len();

    g.set_page_type(a, Some("  Persona ")).unwrap();
    g.set_page_type(b, Some("lugar")).unwrap();
    assert_eq!(g.get_page(a).unwrap().unwrap().types, ["Persona"]);
    assert_eq!(g.get_page(a).unwrap().unwrap().page_type(), Some("Persona"));
    // Changing replaces; types never become pages.
    g.set_page_type(a, Some("empresa")).unwrap();
    assert_eq!(g.get_page(a).unwrap().unwrap().types, ["empresa"]);
    assert_eq!(g.list_pages(true, 100, 0).unwrap().len(), pages_before);
    assert!(g.page_by_title("persona").unwrap().is_none());

    // The type counts as a tag of the same name, once even when also listed.
    assert_eq!(g.pages_with_tag("EMPRESA").unwrap(), [a]);
    g.add_page_tag(a, "Empresa").unwrap();
    g.add_page_tag(a, "web").unwrap();
    let cat = g.tag_catalog().unwrap();
    let count = |n: &str| {
        cat.iter()
            .find(|t| t.name.to_lowercase() == n)
            .map(|t| t.count)
    };
    assert_eq!(
        count("empresa"),
        Some(1),
        "explicit + implicit counted once"
    );
    assert_eq!(count("lugar"), Some(1));
    assert_eq!(count("web"), Some(1));
    assert_eq!(g.all_page_tags().unwrap()[&a].len(), 2);

    // Removing the type removes the implicit tag; the explicit one stays.
    g.set_page_type(a, None).unwrap();
    assert_eq!(g.pages_with_tag("empresa").unwrap(), [a]);
    g.remove_page_tag(a, "empresa").unwrap();
    assert!(g.pages_with_tag("empresa").unwrap().is_empty());
    g.set_page_type(b, None).unwrap();
    assert!(g.pages_with_tag("lugar").unwrap().is_empty());

    let cat = g.type_catalog().unwrap();
    assert!(cat.iter().any(|t| t.name == "empresa" && t.suggested));
    let j = g
        .journal(chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap())
        .unwrap();
    assert!(invalid(g.set_page_type(j.id, Some("x"))));
}

#[test]
fn renaming_and_removing_tags_cover_tags_and_types() {
    let (_d, g) = graph();
    let a = page(&g, "A", &["x"]);
    let b = page(&g, "B", &["y"]);
    let c = page(&g, "C", &["z"]);
    g.add_page_tag(a, "web").unwrap();
    g.add_page_tag(b, "Web").unwrap();
    g.add_page_tag(b, "otro").unwrap();
    g.set_page_type(c, Some("WEB")).unwrap();

    assert_eq!(g.rename_tag("web", "Sitios web").unwrap(), 3);
    assert_eq!(g.get_page(a).unwrap().unwrap().tags, ["Sitios web"]);
    assert_eq!(g.get_page(b).unwrap().unwrap().tags, ["otro", "Sitios web"]);
    assert_eq!(g.get_page(c).unwrap().unwrap().types, ["Sitios web"]);
    assert_eq!(g.pages_with_tag("Sitios web").unwrap().len(), 3);
    assert!(g.pages_with_tag("web").unwrap().is_empty());
    assert!(invalid(g.rename_tag("otro", "  # ")));

    // Removing drops the explicit tag everywhere and leaves types alone.
    assert_eq!(g.remove_tag("sitios web").unwrap(), 2);
    assert!(g.get_page(a).unwrap().unwrap().tags.is_empty());
    assert_eq!(g.get_page(b).unwrap().unwrap().tags, ["otro"]);
    assert_eq!(g.pages_with_tag("Sitios web").unwrap(), [c]);
}

#[test]
fn v5_databases_turn_tag_pages_into_notes() {
    let dir = tempfile::tempdir().unwrap();
    let (tag, member) = {
        let g = Graph::open(dir.path()).unwrap();
        let member = page(&g, "Member", &["see [[Idea]]"]);
        let tag = page(&g, "Idea", &["about ideas"]);
        g.add_page_tag(member, "Idea").unwrap();
        g.set_page_type(member, Some("persona")).unwrap();
        // Simulate schema v5: the tag page has kind 'tag', the member two types.
        g.conn()
            .execute("UPDATE pages SET kind='tag' WHERE id=?1", [tag.to_string()])
            .unwrap();
        g.conn()
            .execute(
                "UPDATE pages SET types='[\"persona\",\"lugar\"]' WHERE id=?1",
                [member.to_string()],
            )
            .unwrap();
        g.conn()
            .execute("UPDATE meta SET value='5' WHERE key='schema'", [])
            .unwrap();
        g.conn().execute("DELETE FROM page_tags", []).unwrap();
        (tag, member)
    };
    let g = Graph::open(dir.path()).unwrap();
    let t = g.get_page(tag).unwrap().unwrap();
    assert_eq!((t.kind, t.title.as_str()), (PageKind::Note, "Idea"));
    assert_eq!(texts(&g, tag), [(0, "about ideas".to_string())]);
    assert_eq!(g.get_page(member).unwrap().unwrap().types, ["persona"]);
    assert_eq!(g.pages_with_tag("idea").unwrap(), [member]);
    assert_eq!(g.pages_with_tag("persona").unwrap(), [member]);
    assert!(g.pages_with_tag("lugar").unwrap().is_empty());
}

#[test]
fn ops_from_old_replicas_degrade_tag_pages_to_notes() {
    let id = synapse_core::new_id();
    let op: Op = serde_json::from_value(json!({
        "type": "upsert_page",
        "page": {"id": id, "title": "Idea", "journal_date": null, "origin_journal": null,
                 "properties": {}, "created_at": 1, "updated_at": 2, "kind": "tag",
                 "tags": [], "types": ["a", "b"]}
    }))
    .unwrap();
    let (_d, g) = graph();
    assert!(
        g.apply_remote("0000000000003-00000-oldactor0001", &op)
            .unwrap()
    );
    let p = g.get_page(id).unwrap().unwrap();
    assert_eq!(p.kind, PageKind::Note);
    assert_eq!(g.pages_with_tag("a").unwrap(), [id]);
    assert!(
        g.pages_with_tag("b").unwrap().is_empty(),
        "only the first type counts"
    );
}

#[test]
fn tags_and_type_sync_with_the_page() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    let p = page(&a, "Note", &["x"]);
    a.add_page_tag(p, "proyecto").unwrap();
    a.add_page_tag(p, "idea").unwrap();
    a.set_page_type(p, Some("concepto")).unwrap();
    ship(&a, &b, 0);
    let q = b.get_page(p).unwrap().unwrap();
    assert_eq!(q.tags, ["idea", "proyecto"]);
    assert_eq!(q.types, ["concepto"]);
    assert_eq!(b.pages_with_tag("IDEA").unwrap(), [p]);
    assert_eq!(b.pages_with_tag("concepto").unwrap(), [p]);
    assert!(
        b.page_by_title("idea").unwrap().is_none(),
        "no tag pages travel"
    );
    b.rename_tag("idea", "ocurrencia").unwrap();
    ship(&b, &a, 0);
    assert_eq!(
        a.get_page(p).unwrap().unwrap().tags,
        ["ocurrencia", "proyecto"]
    );
}

#[test]
fn v5_migration_sorts_tags_and_survives_malformed_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (a, bad, bad2) = {
        let g = Graph::open(dir.path()).unwrap();
        let a = page(&g, "A", &["x"]);
        let bad = page(&g, "Bad", &["x"]);
        let bad2 = page(&g, "Bad2", &["x"]);
        let set = |id: PageId, tags: &str, types: &str| {
            g.conn()
                .execute(
                    "UPDATE pages SET tags=?2, types=?3 WHERE id=?1",
                    [id.to_string(), tags.to_string(), types.to_string()],
                )
                .unwrap();
        };
        set(a, "[\"zeta\",\"Alfa\",\"alfa\"]", "[\"persona\"]");
        set(bad, "not json", "[]");
        set(bad2, "[\"ok\"]", "{oops");
        g.conn()
            .execute("UPDATE meta SET value='5' WHERE key='schema'", [])
            .unwrap();
        (a, bad, bad2)
    };
    let g = Graph::open(dir.path()).unwrap();
    assert_eq!(g.get_page(a).unwrap().unwrap().tags, ["Alfa", "zeta"]);
    assert_eq!(g.pages_with_tag("alfa").unwrap(), [a]);
    assert_eq!(g.pages_with_tag("persona").unwrap(), [a]);
    assert!(g.get_page(bad).unwrap().unwrap().tags.is_empty());
    let p = g.get_page(bad2).unwrap().unwrap();
    assert_eq!((p.tags, p.types), (vec!["ok".to_string()], vec![]));
    assert_eq!(g.pages_with_tag("ok").unwrap(), [bad2]);
}

#[test]
fn remote_upserts_store_tags_sorted() {
    let id = synapse_core::new_id();
    let op: Op = serde_json::from_value(json!({
        "type": "upsert_page",
        "page": {"id": id, "title": "N", "journal_date": null, "origin_journal": null,
                 "properties": {}, "created_at": 1, "updated_at": 2, "kind": "note",
                 "tags": ["zeta", "Beta", "beta", "alfa"], "types": []}
    }))
    .unwrap();
    let (_d, g) = graph();
    assert!(
        g.apply_remote("0000000000003-00000-oldactor0001", &op)
            .unwrap()
    );
    assert_eq!(
        g.get_page(id).unwrap().unwrap().tags,
        ["alfa", "Beta", "zeta"]
    );
}

#[test]
fn a_direct_hub_is_an_implicit_tag_of_its_members() {
    let (_d, g) = graph();
    let anime = hub(&g, "Anime");
    let media = hub(&g, "Media");
    g.add_to_hub(anime, media).unwrap();
    let a = page(&g, "A", &["x"]);
    let b = page(&g, "B", &["x"]);
    g.add_to_hub(a, anime).unwrap();
    g.add_to_hub(b, anime).unwrap();
    // The hub tag is not stored in `tags`, ancestors do not count, hubs and templates get none.
    assert!(g.get_page(a).unwrap().unwrap().tags.is_empty());
    let mut got = g.pages_with_tag("anime").unwrap();
    got.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(got, want);
    assert!(g.pages_with_tag("Media").unwrap().is_empty());
    let tmpl = g.hub_template(anime).unwrap();
    assert!(!g.pages_with_tag("Anime").unwrap().contains(&tmpl));
    assert!(!g.pages_with_tag("Anime").unwrap().contains(&anime));

    // Merges with a same-name type: one tag, the page counted once.
    g.set_page_type(a, Some("anime")).unwrap();
    let cat = g.tag_catalog().unwrap();
    assert_eq!(
        cat.iter()
            .filter(|t| t.name.to_lowercase() == "anime")
            .count(),
        1
    );
    assert_eq!(
        cat.iter()
            .find(|t| t.name.to_lowercase() == "anime")
            .unwrap()
            .count,
        2
    );
    assert_eq!(g.all_page_tags().unwrap()[&a], ["anime"]);

    // Renaming the hub re-indexes its members without touching them.
    g.rename_page(anime, "Series").unwrap();
    assert!(
        g.pages_with_tag("Anime").unwrap().contains(&a),
        "type still implies it"
    );
    assert!(!g.pages_with_tag("Anime").unwrap().contains(&b));
    assert!(g.pages_with_tag("Series").unwrap().contains(&b));

    // Moving a page between hubs moves the implicit tag; leaving the hub drops it.
    g.add_to_hub(b, media).unwrap();
    assert!(!g.pages_with_tag("Series").unwrap().contains(&b));
    assert_eq!(g.pages_with_tag("Media").unwrap(), [b]);
    g.remove_from_hub(b, media).unwrap();
    assert!(g.pages_with_tag("Media").unwrap().is_empty());

    // rename_tag never renames the hub; only explicit tags and types.
    g.add_to_hub(b, media).unwrap();
    assert_eq!(g.rename_tag("Media", "Press").unwrap(), 0);
    assert_eq!(g.get_page(media).unwrap().unwrap().title, "Media");
    assert_eq!(g.pages_with_tag("Media").unwrap(), [b]);

    // Deleting the hub removes the tag from its members.
    g.delete_page(media).unwrap();
    assert!(g.pages_with_tag("Media").unwrap().is_empty());
}

#[test]
fn v6_databases_gain_hub_tags() {
    let dir = tempfile::tempdir().unwrap();
    let a = {
        let g = Graph::open(dir.path()).unwrap();
        let h = hub(&g, "Anime");
        let a = page(&g, "A", &["x"]);
        g.add_to_hub(a, h).unwrap();
        g.conn().execute("DELETE FROM page_tags", []).unwrap();
        g.conn()
            .execute("UPDATE meta SET value='6' WHERE key='schema'", [])
            .unwrap();
        a
    };
    let g = Graph::open(dir.path()).unwrap();
    assert_eq!(g.pages_with_tag("anime").unwrap(), [a]);
}
