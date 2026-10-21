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
