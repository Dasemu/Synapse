use synapse_core::{Graph, Op};

fn graph() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    (dir, g)
}

fn module_ops(g: &Graph) -> Vec<(String, Op)> {
    g.ops_since(0, 10_000)
        .unwrap()
        .into_iter()
        .filter(|r| matches!(r.op, Op::PutModuleData { .. }))
        .map(|r| (r.hlc, r.op))
        .collect()
}

#[test]
fn module_data_is_namespaced_listed_and_deleted() {
    let (_d, g) = graph();
    g.set_module_data("a.mod", "entry/1", Some("x")).unwrap();
    g.set_module_data("a.mod", "entry/2", Some("y")).unwrap();
    g.set_module_data("a.mod", "vault", Some("h")).unwrap();
    g.set_module_data("b.mod", "entry/1", Some("other")).unwrap();
    assert_eq!(g.module_data("a.mod", "entry/1").unwrap().as_deref(), Some("x"));
    let keys: Vec<_> = g
        .module_data_list("a.mod", "entry/")
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, ["entry/1", "entry/2"]);
    g.set_module_data("a.mod", "entry/1", None).unwrap();
    assert_eq!(g.module_data("a.mod", "entry/1").unwrap(), None);
    assert_eq!(g.module_data_list("a.mod", "").unwrap().len(), 2);
    // Deleting a missing key logs nothing.
    let before = module_ops(&g).len();
    g.set_module_data("a.mod", "missing", None).unwrap();
    assert_eq!(module_ops(&g).len(), before);
    assert_eq!(g.clear_module_data("a.mod").unwrap(), 2);
    assert!(g.module_data_list("a.mod", "").unwrap().is_empty());
    assert_eq!(g.module_data_list("b.mod", "").unwrap().len(), 1);
}

#[test]
fn module_data_validates_ids_keys_and_sizes() {
    let (_d, g) = graph();
    assert!(g.set_module_data("Bad Id", "k", Some("v")).is_err());
    assert!(g.set_module_data("ok", "", Some("v")).is_err());
    assert!(g.set_module_data("ok", "a\nb", Some("v")).is_err());
    let big = "x".repeat(synapse_core::MAX_MODULE_VALUE + 1);
    assert!(g.set_module_data("ok", "k", Some(&big)).is_err());
}

#[test]
fn module_data_syncs_last_writer_wins_with_tombstones() {
    let (_a, a) = graph();
    let (_b, b) = graph();
    a.set_module_data("m", "k", Some("first")).unwrap();
    for (hlc, op) in module_ops(&a) {
        assert!(b.apply_remote(&hlc, &op).unwrap());
    }
    assert_eq!(b.module_data("m", "k").unwrap().as_deref(), Some("first"));
    // A later delete on b wins over a's older write replayed again.
    b.set_module_data("m", "k", None).unwrap();
    for (hlc, op) in module_ops(&a) {
        assert!(!b.apply_remote(&hlc, &op).unwrap());
    }
    assert_eq!(b.module_data("m", "k").unwrap(), None);
    // ...and reaches a.
    let ops = module_ops(&b);
    let (hlc, op) = ops.last().unwrap();
    assert!(a.apply_remote(hlc, op).unwrap());
    assert_eq!(a.module_data("m", "k").unwrap(), None);
    // Invalid remote ops are ignored.
    let bad = Op::PutModuleData {
        module: "Bad Id".into(),
        key: "k".into(),
        value: Some("v".into()),
    };
    assert!(!a.apply_remote("0000000000001-00000-x", &bad).unwrap());
}
