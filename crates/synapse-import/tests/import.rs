use std::path::{Path, PathBuf};
use synapse_core::Graph;
use synapse_import::{ExistingPages, ImportOptions, Phase, Report, import_logseq};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph")
}

fn run(g: &Graph, opts: &ImportOptions) -> Report {
    import_logseq(&fixture(), g, opts, |_| {}).unwrap()
}

fn texts(g: &Graph, title: &str) -> Vec<(usize, String)> {
    let p = g.page_by_title(title).unwrap().expect(title);
    g.page_tree(p.id)
        .unwrap()
        .into_iter()
        .map(|(d, b)| (d, b.content))
        .collect()
}

#[test]
fn imports_fixture_graph() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let mut phases = Vec::new();
    let r = import_logseq(&fixture(), &g, &ImportOptions::default(), |p| {
        phases.push(p.phase)
    })
    .unwrap();
    assert!(phases.contains(&Phase::Assets) && phases.contains(&Phase::Blocks));
    assert_eq!(r.journals, 2);
    assert_eq!(r.pages, 6);
    assert_eq!(r.assets, 2);
    assert_eq!(r.org_files, vec!["pages/Legacy.org"]);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("missing asset nope.png"))
    );

    // namespace from file name, nesting
    let t = texts(&g, "Project/Alpha");
    assert_eq!(t[0], (0, "Root of the project".into()));
    assert_eq!(t[2], (1, "TODO write the importer".into()));

    // title:: override
    assert!(g.page_by_title("Fancy Title: Q&A").unwrap().is_some());

    // properties, aliases, block id kept, collapsed
    let an = g.page_by_title("Alpha Notes").unwrap().unwrap();
    assert_eq!(an.properties["type"], "[[Note]]");
    assert_eq!(g.page_by_title("an").unwrap().unwrap().id, an.id);
    assert_eq!(g.page_by_title("Alpha Alias").unwrap().unwrap().id, an.id);
    let tree = g.page_tree(an.id).unwrap();
    let first = g
        .get_block("11111111-1111-4111-8111-111111111111".parse().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(first.content, "First block");
    assert_eq!(tree[1].1.content, "Second\nwith a continuation line");
    assert!(tree[1].1.collapsed);
    assert_eq!(tree[2].1.parent_id, Some(tree[1].1.id));

    // block refs keep working
    let b = g.block_backrefs(first.id).unwrap();
    assert_eq!(b.len(), 1);

    // assets rewritten, original name kept
    let media = texts(&g, "Media");
    let hash = blake3_hex(b"PNGDATA");
    assert_eq!(media[0].1, format!("Picture ![diagram](asset://{hash})"));
    assert!(media[2].1.contains("../assets/nope.png"));
    assert!(media[3].1.contains("- not a bullet"));
    let a = g.asset(&hash).unwrap().unwrap();
    assert_eq!(a.original_name.as_deref(), Some("pic one.png"));
    assert!(g.blobs().contains(&hash));

    // org raw
    let org = texts(&g, "Legacy");
    assert_eq!(org.len(), 2);
    assert!(org[0].1.starts_with("* Heading one"));

    // journals: date links normalised, origin = earliest referencing journal
    let j = g.page_by_title("2024-03-15").unwrap().unwrap();
    assert!(j.is_journal());
    let jt = texts(&g, "2024-03-15");
    assert!(jt[1].1.contains("[[2024-03-16]]"));
    let ideas = g.page_by_title("Ideas").unwrap().unwrap();
    assert_eq!(
        ideas.origin_journal,
        chrono::NaiveDate::from_ymd_opt(2024, 3, 10)
    );
    // alias reference counts for the aliased page's origin
    assert_eq!(
        an.origin_journal,
        chrono::NaiveDate::from_ymd_opt(2024, 3, 10)
    );
    // timestamps come from the file
    let mtime = std::fs::metadata(fixture().join("pages/Ideas.md"))
        .unwrap()
        .modified()
        .unwrap();
    let ms = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert_eq!(ideas.updated_at, ms);
}

#[test]
fn reimport_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    run(&g, &ImportOptions::default());
    let pages = g.list_pages(true, 1000, 0).unwrap().len();
    let ops = g.last_seq().unwrap();
    let r = run(&g, &ImportOptions::default());
    assert_eq!((r.pages, r.journals, r.blocks, r.updated), (0, 0, 0, 0));
    assert_eq!(r.unchanged, 8);
    assert_eq!(g.list_pages(true, 1000, 0).unwrap().len(), pages);
    // only aliases / asset dedup may log; no block or page ops
    let new_ops = g.ops_since(ops, 1000).unwrap();
    assert!(
        new_ops.iter().all(|o| !matches!(
            o.op,
            synapse_core::Op::UpsertBlock { .. } | synapse_core::Op::UpsertPage { .. }
        )),
        "{new_ops:?}"
    );
}

#[test]
fn reimport_updates_changed_pages_and_skip_mode() {
    let src = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), src.path());
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let o = ImportOptions::default();
    import_logseq(src.path(), &g, &o, |_| {}).unwrap();

    std::fs::write(
        src.path().join("pages/Ideas.md"),
        "- Idea one\n- Idea three\n",
    )
    .unwrap();
    let skip = ImportOptions {
        existing: ExistingPages::Skip,
    };
    let r = import_logseq(src.path(), &g, &skip, |_| {}).unwrap();
    assert_eq!(r.updated, 0);
    assert_eq!(texts(&g, "Ideas")[1].1, "Idea two");

    let r = import_logseq(src.path(), &g, &o, |_| {}).unwrap();
    assert_eq!((r.pages, r.updated), (1, 1));
    let t = texts(&g, "Ideas");
    assert_eq!(t.len(), 2);
    assert_eq!(t[1].1, "Idea three");
}

#[test]
fn rejects_non_graph() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path().join("g")).unwrap();
    assert!(import_logseq(dir.path(), &g, &ImportOptions::default(), |_| {}).is_err());
}

fn blake3_hex(data: &[u8]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let store = synapse_core::blob::BlobStore::open(dir.path()).unwrap();
    store.put_reader(data).unwrap().0
}

fn copy_dir(from: &Path, to: &Path) {
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            std::fs::create_dir_all(&dst).unwrap();
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn mini_graph(files: &[(&str, &str)]) -> tempfile::TempDir {
    let src = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        let p = src.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    src
}

fn imp(src: &Path, g: &Graph) -> Report {
    import_logseq(src, g, &ImportOptions::default(), |_| {}).unwrap()
}

#[test]
fn native_blocks_survive_import_and_reimport() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let page = g.ensure_page("Notes", None).unwrap();
    g.insert_block(
        page.id,
        synapse_core::Placement::LastChildOf(None),
        "native",
    )
    .unwrap();
    let today = g.today_journal().unwrap();
    g.insert_block(
        today.id,
        synapse_core::Placement::LastChildOf(None),
        "today native",
    )
    .unwrap();
    let src = mini_graph(&[("pages/Notes.md", "- from logseq\n- second\n")]);

    imp(src.path(), &g);
    let t = texts(&g, "Notes");
    assert_eq!(t[0].1, "native");
    assert_eq!(t[1].1, "from logseq", "imported blocks are appended");

    // file drops a block, user edited another one in Synapse
    let second = g.page_tree(page.id).unwrap()[2].1.clone();
    g.update_block(second.id, "second (edited in Synapse)")
        .unwrap();
    std::fs::write(src.path().join("pages/Notes.md"), "- from logseq changed\n").unwrap();
    let r = imp(src.path(), &g);
    let t: Vec<String> = texts(&g, "Notes").into_iter().map(|x| x.1).collect();
    assert!(t.contains(&"native".to_string()));
    assert!(
        t.contains(&"second (edited in Synapse)".to_string()),
        "{t:?}"
    );
    assert!(t.contains(&"from logseq changed".to_string()));
    assert!(!t.contains(&"from logseq".to_string()));
    assert!(
        r.warnings.iter().any(|w| w.contains("edited")),
        "{:?}",
        r.warnings
    );
    assert_eq!(
        texts(&g, &synapse_core::journal_title(g.today())),
        vec![(0, "today native".to_string())]
    );
}

#[test]
fn imported_journal_merges_with_native_today() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let today = g.today_journal().unwrap();
    g.insert_block(today.id, synapse_core::Placement::LastChildOf(None), "mine")
        .unwrap();
    let name = g.today().format("%Y_%m_%d").to_string();
    let src = mini_graph(&[(&format!("journals/{name}.md"), "- old entry\n")]);
    imp(src.path(), &g);
    imp(src.path(), &g);
    let t: Vec<String> = texts(&g, &synapse_core::journal_title(g.today()))
        .into_iter()
        .map(|x| x.1)
        .collect();
    assert_eq!(t, vec!["mine", "old entry"]);
}

#[test]
fn failing_document_is_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    g.conn()
        .execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON blocks WHEN NEW.content='boom'
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
    let src = mini_graph(&[
        ("pages/Bad.md", "- fine\n- boom\n"),
        ("pages/Good.md", "- ok\n"),
    ]);
    let r = imp(src.path(), &g);
    assert!(
        r.warnings.iter().any(|w| w.contains("Bad.md")),
        "{:?}",
        r.warnings
    );
    assert!(
        texts(&g, "Bad").is_empty(),
        "partial blocks must not be committed"
    );
    assert_eq!(texts(&g, "Good").len(), 1);
}

#[test]
fn inserting_a_block_keeps_other_ids_stable() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let src = mini_graph(&[("pages/P.md", "- a\n\t- a1\n- b\n- b\n")]);
    imp(src.path(), &g);
    let ids = |g: &Graph| -> Vec<(String, uuid::Uuid)> {
        let p = g.page_by_title("P").unwrap().unwrap();
        g.page_tree(p.id)
            .unwrap()
            .into_iter()
            .map(|(_, b)| (b.content, b.id))
            .collect()
    };
    let before = ids(&g);
    std::fs::write(
        src.path().join("pages/P.md"),
        "- NEW\n- a\n\t- a1\n- b\n- b\n",
    )
    .unwrap();
    imp(src.path(), &g);
    let after = ids(&g);
    assert_eq!(after.len(), 5);
    for old in &before {
        assert!(after.contains(old), "{old:?} changed id: {after:?}");
    }
}

#[test]
fn renamed_page_is_not_duplicated() {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let src = mini_graph(&[("pages/Old.md", "- one\n")]);
    imp(src.path(), &g);
    let p = g.page_by_title("Old").unwrap().unwrap();
    g.rename_page(p.id, "Renamed").unwrap();
    std::fs::write(src.path().join("pages/Old.md"), "- one\n- two\n").unwrap();
    imp(src.path(), &g);
    assert!(g.page_by_title("Old").unwrap().is_none());
    let t = texts(&g, "Renamed");
    assert_eq!(t.len(), 2);
}
