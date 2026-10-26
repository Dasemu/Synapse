use synapse_ffi::api::dto::{BlockDto, Place, PlaceKind, edge_kind};
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

    // Outdenting `c` adopts `d` as its child (direct outdenting).
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

#[test]
fn hubs_structure_and_graph_nodes() {
    let (_d, g) = open();
    let media = g.create_hub("Media".into(), " All media ".into()).unwrap();
    assert!(media.is_hub);
    assert_eq!(media.description, "All media");
    let media = media.id;
    let books = g
        .create_hub("Media/Books".into(), String::new())
        .unwrap()
        .id;
    let (novel, _) = page_with(&g, "Novel", &["text"]);
    let (film, _) = page_with(&g, "Film", &["text"]);
    let (note, _) = page_with(&g, "Note", &["plain text"]);
    g.add_to_hub(books.clone(), media.clone()).unwrap();
    g.add_to_hub(film.clone(), media.clone()).unwrap();
    g.add_to_hub(novel.clone(), books.clone()).unwrap();
    g.add_to_hub(novel.clone(), books.clone()).unwrap(); // no-op

    // Rules surface as readable errors.
    let err = g.add_to_hub(media.clone(), books.clone()).unwrap_err();
    assert!(err.to_string().contains("cycle"), "{err}");
    assert!(
        g.add_to_hub(note.clone(), film.clone()).is_err(),
        "not a hub"
    );
    assert!(g.add_to_hub(media.clone(), media.clone()).is_err(), "self");
    let today = g.open_page(g.today()).unwrap().page.id;
    assert!(g.add_to_hub(today, media.clone()).is_err(), "journal");
    let err = g
        .insert_block(
            media.clone(),
            place(PlaceKind::LastChildOf, None),
            "x".into(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("hubs have no blocks"), "{err}");

    let hubs = g.hubs().unwrap();
    let titles: Vec<&str> = hubs.hubs.iter().map(|h| h.title.as_str()).collect();
    assert_eq!(titles, ["Media", "Media/Books"]);
    assert_eq!(hubs.roots, std::slice::from_ref(&media));
    let m = &hubs.hubs[0];
    assert_eq!((m.depth, m.description.as_str()), (0, "All media"));
    let names: Vec<&str> = m.members.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(names, ["Media/Books", "Film"], "sub-hubs first");
    assert!(m.members[0].is_hub && !m.members[1].is_hub);
    assert_eq!(m.sub_hubs, std::slice::from_ref(&books));
    assert_eq!(hubs.hubs[1].depth, 1);

    let s = g.page_structure(novel.clone()).unwrap();
    assert!(!s.is_hub && s.members.is_empty());
    let in_hubs: Vec<&str> = s.in_hubs.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(in_hubs, ["Media/Books"], "one hub per page");
    let s = g.page_structure(books.clone()).unwrap();
    assert!(s.is_hub);
    assert_eq!(s.members.len(), 1);
    assert_eq!(s.ns_path.len(), 1);
    assert_eq!(s.ns_path[0].id.as_deref(), Some(media.as_str()));
    let s = g.page_structure(media.clone()).unwrap();
    assert_eq!(s.description, "All media");
    assert_eq!(s.ns_children[0].id.as_deref(), Some(books.as_str()));

    g.set_hub_description(books.clone(), "Reading".into())
        .unwrap();
    g.remove_from_hub(film.clone(), media.clone()).unwrap();
    assert!(g.page_structure(film.clone()).unwrap().in_hubs.is_empty());

    // Graph data: hub flags, home hubs and CONTAINS edges.
    let data = g.graph_data().unwrap();
    let idx = |id: &str| data.nodes.iter().position(|n| n.id == id).unwrap() as u32;
    let node = |id: &str| &data.nodes[idx(id) as usize];
    assert!(node(&media).is_hub && node(&books).is_hub && !node(&novel).is_hub);
    assert_eq!(node(&books).home_hub, Some(idx(&books)));
    assert_eq!(node(&novel).home_hub, Some(idx(&books)), "deepest hub");
    assert_eq!(node(&note).home_hub, None);
    let contains = |s: &str, t: &str| {
        (0..data.kinds.len()).any(|e| {
            data.kinds[e] == edge_kind::CONTAINS
                && data.sources[e] == idx(s)
                && data.targets[e] == idx(t)
        })
    };
    assert!(contains(&media, &books) && contains(&books, &novel));
    assert!(!contains(&media, &novel));
    assert!(!contains(&media, &film));
    assert_eq!(node(&novel).backlinks, 1, "membership is structural");

    // Converting back to a note: description, then one link per member.
    let n = g.convert_to_note(books.clone()).unwrap();
    assert!(!n.is_hub);
    let page = g.page(books.clone()).unwrap().unwrap();
    let texts: Vec<&str> = page.blocks.iter().map(|b| b.content.as_str()).collect();
    assert_eq!(texts, ["Reading", "[[Novel]]"]);
    let in_hubs = g.page_structure(novel.clone()).unwrap().in_hubs;
    assert!(in_hubs.is_empty(), "the old hub is gone from its members");
}

#[test]
fn convert_note_to_hub() {
    let (_d, g) = open();
    let (area, _) = page_with(
        &g,
        "Work/Area",
        &[
            "Everything about **my** area of [[Work]].",
            "[[Alpha]]",
            "[[Beta]] #Gamma",
        ],
    );
    let p = g.hub_conversion_preview(area.clone()).unwrap();
    assert_eq!(p.suggested_title, "Area");
    assert_eq!(p.suggested_note_title, "Area (notes)");
    assert_eq!(p.suggested_description, "Everything about my area of Work.");
    let names: Vec<&str> = p.members.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(names, ["Work", "Alpha", "Beta", "Gamma"]);
    assert_eq!((p.prose_blocks, p.blocks), (1, 3));

    let member_ids: Vec<String> = p.members.iter().skip(1).map(|m| m.id.clone()).collect();
    let hub = g
        .convert_to_hub(
            area.clone(),
            p.suggested_title.clone(),
            p.suggested_description.clone(),
            member_ids,
            Some(p.suggested_note_title.clone()),
        )
        .unwrap();
    assert!(hub.is_hub);
    assert_eq!(
        (hub.id.as_str(), hub.title.as_str()),
        (area.as_str(), "Area")
    );
    assert!(g.page(area.clone()).unwrap().unwrap().blocks.is_empty());
    let kept = g.page_by_title("Area (notes)".into()).unwrap().unwrap();
    assert_eq!(kept.blocks.len(), 3);
    let s = g.page_structure(area.clone()).unwrap();
    let names: Vec<&str> = s.members.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        names,
        ["Alpha", "Area (notes)", "Beta", "Gamma"],
        "kept note is a member"
    );

    // A blank keep title is an error (never "delete the outline") and changes nothing.
    let (blank, _) = page_with(&g, "Blank", &["keep me"]);
    assert!(
        g.convert_to_hub(
            blank.clone(),
            "Blank".into(),
            String::new(),
            vec![],
            Some(" ".into())
        )
        .is_err()
    );
    assert!(!g.page(blank.clone()).unwrap().unwrap().page.is_hub);
    assert_eq!(g.page(blank).unwrap().unwrap().blocks.len(), 1);

    // A title clash is a readable error and changes nothing.
    let (other, _) = page_with(&g, "Other", &["[[Alpha]]"]);
    let err = g
        .convert_to_hub(other.clone(), "Area".into(), String::new(), vec![], None)
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    assert!(!g.page(other.clone()).unwrap().unwrap().page.is_hub);
    assert_eq!(g.page(other).unwrap().unwrap().blocks.len(), 1);
}

#[test]
fn legacy_candidates_and_purge() {
    let (_d, g) = open();
    let (todo, _) = page_with(&g, "TODO", &["a", "b"]);
    let (hl, _) = page_with(&g, "hls__book", &[]);
    let (sys, _) = page_with(&g, "Template", &["x"]);
    let (keep, _) = page_with(&g, "Keep", &["x"]);
    page_with(&g, "contents", &["- x"]);
    page_with(&g, "now", &[]);
    g.create_hub("later".into(), String::new()).unwrap();
    {
        let shared = g.shared();
        let core = shared.lock().unwrap();
        let mut props = synapse_core::Properties::new();
        props.insert("type".into(), "system".into());
        core.set_page_properties(sys.parse().unwrap(), props)
            .unwrap();
    }

    let c = g.legacy_candidates().unwrap();
    let by = |t: &str| c.iter().find(|x| x.title == t).unwrap();
    assert_eq!(by("TODO").reason, "Logseq built-in");
    assert!(
        !by("TODO").preselected && by("TODO").blocks == 2,
        "has content"
    );
    assert!(by("contents").preselected, "organisational page");
    assert!(by("now").preselected, "empty built-in");
    assert!(!by("later").preselected, "hubs are never preselected");
    assert_eq!(by("hls__book").reason, "Logseq PDF highlights");
    assert!(by("hls__book").preselected);
    assert!(!by("Template").preselected);
    assert_eq!(by("Template").reason, "type:: system");
    assert!(c.iter().all(|x| x.title != "Keep"));

    // A bad id rejects the call before anything is deleted.
    assert!(g.purge_pages(vec![todo.clone(), "nope".into()]).is_err());
    assert!(g.page(todo.clone()).unwrap().is_some());
    assert_eq!(g.purge_pages(vec![todo.clone(), hl.clone()]).unwrap(), 2);
    assert!(g.page(todo).unwrap().is_none() && g.page(hl).unwrap().is_none());
    assert!(g.page(keep).unwrap().is_some());
}

#[test]
fn tags_are_virtual_graph_nodes_with_their_own_edge_kind() {
    let (_d, g) = open();
    let (a, _) = page_with(&g, "Rust", &["x"]);
    let (b, _) = page_with(&g, "Go", &["y"]);
    let (c, _) = page_with(&g, "Madrid", &["z"]);
    for (p, tags) in [
        (&a, vec!["tecnología", "idea"]),
        (&b, vec!["Tecnología", "idea"]),
    ] {
        g.set_page_tags(p.clone(), tags.into_iter().map(String::from).collect())
            .unwrap();
    }
    g.set_page_type(c.clone(), Some("lugar".into())).unwrap();
    g.set_page_type(b.clone(), Some("Idea".into())).unwrap();
    let data = g.graph_data().unwrap();
    let title = |i: u32| data.nodes[i as usize].title.clone();
    let mut tagged: Vec<(String, String)> = (0..data.kinds.len())
        .filter(|&e| data.kinds[e] == edge_kind::TAGGED)
        .map(|e| (title(data.sources[e]), title(data.targets[e])))
        .collect();
    tagged.sort();
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(
        tagged,
        [
            pair("Go", "idea"),
            pair("Go", "tecnología"),
            pair("Madrid", "lugar"),
            pair("Rust", "idea"),
            pair("Rust", "tecnología"),
        ],
        "each page points at its tags, including the one its type implies, once"
    );
    // Tags are virtual: no page behind them, flagged, with the page count as degree.
    let tag = data
        .nodes
        .iter()
        .find(|n| n.is_tag && n.title == "tecnología");
    let tag = tag.expect("tag node");
    assert_eq!(tag.id, "tag:tecnología");
    assert_eq!((tag.degree, tag.journal_date.clone()), (2, None));
    assert!(g.page(tag.id.clone()).is_err() || g.page(tag.id.clone()).unwrap().is_none());
    assert_eq!(data.nodes.iter().filter(|n| n.is_tag).count(), 3);
    let go = data.nodes.iter().find(|n| n.title == "Go").unwrap();
    assert_eq!(
        go.tags,
        ["idea", "Tecnología"],
        "tags, with the type's tag once"
    );
    let cat = g.tag_catalog().unwrap();
    assert_eq!((cat[0].name.as_str(), cat[0].count), ("idea", 2));
    assert_eq!(g.pages_with_tag("IDEA".into()).unwrap().len(), 2);
}

#[test]
fn page_type_and_tag_renaming_round_trip_through_the_api() {
    let (_d, g) = open();
    let (a, _) = page_with(&g, "Ana", &["x"]);
    let p = g.set_page_type(a.clone(), Some("persona".into())).unwrap();
    assert_eq!(p.page_type.as_deref(), Some("persona"));
    let p = g.set_page_type(a.clone(), Some("lugar".into())).unwrap();
    assert_eq!(p.page_type.as_deref(), Some("lugar"));
    assert!(g.type_catalog().unwrap().iter().any(|t| t.name == "lugar"));
    g.add_page_tag(a.clone(), "web".into()).unwrap();
    assert_eq!(g.rename_tag("lugar".into(), "sitio".into()).unwrap(), 1);
    assert_eq!(g.rename_tag("web".into(), "red".into()).unwrap(), 1);
    let p = g.page(a.clone()).unwrap().unwrap().page;
    assert_eq!(
        (p.page_type.as_deref(), p.tags),
        (Some("sitio"), vec!["red".to_string()])
    );
    assert_eq!(g.remove_tag("red".into()).unwrap(), 1);
    assert!(g.set_page_type(a, None).unwrap().page_type.is_none());
}
