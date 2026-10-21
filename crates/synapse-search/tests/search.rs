use synapse_core::{Graph, NewPage, Placement, journal_title};
use synapse_search::testing::HashEmbedder;
use synapse_search::{
    Filter, HitKind, JournalFilter, LinkMatcher, Reason, Scope, Search, Stemming, apply_suggestion,
};
use tempfile::TempDir;

fn setup() -> (TempDir, Graph) {
    let dir = TempDir::new().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    (dir, g)
}

fn add(g: &Graph, page: &str, text: &str) -> synapse_core::Block {
    let p = g.ensure_page(page, None).unwrap();
    g.insert_block(p.id, Placement::LastChildOf(None), text)
        .unwrap()
}

#[test]
fn fulltext_folding_prefix_fuzzy_and_incremental() {
    let (_d, g) = setup();
    let b = add(&g, "Music", "Mi canción favorita es de Ñandú");
    add(&g, "Cooking", "Receta de paella valenciana");
    let mut s = Search::open(&g).unwrap();
    let st = s.sync(&g).unwrap();
    assert!(st.blocks_indexed >= 2);

    let f = Filter::default();
    macro_rules! hit {
        ($q:expr) => {
            s.search(&g, $q, &f, 10).unwrap()
        };
    }
    assert_eq!(hit!("cancion")[0].id, b.id, "ascii folding");
    assert_eq!(hit!("CANCIÓN")[0].id, b.id);
    assert_eq!(hit!("nandu")[0].id, b.id);
    assert_eq!(hit!("favo")[0].id, b.id, "prefix on last token");
    assert_eq!(hit!("favorta")[0].id, b.id, "typo tolerance");
    assert!(hit!("zzzz").is_empty());

    // Incremental: edit, delete.
    g.update_block(b.id, "Ahora hablo de guitarras").unwrap();
    s.sync(&g).unwrap();
    assert!(hit!("cancion").is_empty());
    assert_eq!(hit!("guitarras")[0].id, b.id);
    g.delete_block(b.id).unwrap();
    s.sync(&g).unwrap();
    assert!(hit!("guitarras").is_empty());

    // Quick switcher over titles and aliases.
    let p = g.page_by_title("Cooking").unwrap().unwrap();
    g.set_alias("Cocina", Some(p.id)).unwrap();
    s.sync(&g).unwrap();
    let q = s.quick_switch(&g, "coc", 5).unwrap();
    assert_eq!(q[0].kind, HitKind::Page);
    assert_eq!(q[0].page_id, p.id);
    g.rename_page(p.id, "Recetas").unwrap();
    s.sync(&g).unwrap();
    assert_eq!(s.quick_switch(&g, "rece", 5).unwrap()[0].page_id, p.id);
}

#[test]
fn state_persists_and_rebuild_works() {
    let (d, g) = setup();
    let b = add(&g, "A", "hello persistent world");
    {
        let mut s = Search::open(&g).unwrap();
        s.sync(&g).unwrap();
    }
    let mut s = Search::open(&g).unwrap();
    assert_eq!(s.sync(&g).unwrap().ops_processed, 0);
    assert_eq!(
        s.search(&g, "persistent", &Filter::default(), 5).unwrap()[0].id,
        b.id
    );
    s.rebuild(&g).unwrap();
    assert_eq!(
        s.search(&g, "persistent", &Filter::default(), 5).unwrap()[0].id,
        b.id
    );
    drop(s);
    // Index directory wiped: reopening recovers from the op log.
    std::fs::remove_dir_all(d.path().join("index")).unwrap();
    let mut s = Search::open(&g).unwrap();
    s.sync(&g).unwrap();
    assert_eq!(
        s.search(&g, "persistent", &Filter::default(), 5).unwrap()[0].id,
        b.id
    );
}

#[test]
fn stemming_and_filters() {
    let (_d, g) = setup();
    let j = g
        .journal(chrono::NaiveDate::from_ymd_opt(2024, 3, 5).unwrap())
        .unwrap();
    let jb = g
        .insert_block(
            j.id,
            Placement::LastChildOf(None),
            "corriendo por el parque",
        )
        .unwrap();
    let pb = add(&g, "Deporte", "correr es sano");
    let mut s = Search::open_with(
        &g,
        synapse_search::Config {
            stemming: Stemming::Spanish,
            ..Default::default()
        },
    )
    .unwrap();
    s.sync(&g).unwrap();
    let all = s.search(&g, "correr", &Filter::default(), 10).unwrap();
    assert_eq!(
        all.len(),
        2,
        "stemmed: correr/corriendo share a stem? {all:?}"
    );

    let only_j = Filter {
        journals: JournalFilter::Only,
        ..Default::default()
    };
    assert_eq!(s.search(&g, "correr", &only_j, 10).unwrap()[0].id, jb.id);
    let no_j = Filter {
        journals: JournalFilter::Exclude,
        ..Default::default()
    };
    assert_eq!(s.search(&g, "correr", &no_j, 10).unwrap()[0].id, pb.id);
    let range = Filter {
        from: chrono::NaiveDate::from_ymd_opt(2024, 3, 1),
        to: chrono::NaiveDate::from_ymd_opt(2024, 3, 31),
        ..Default::default()
    };
    let r = s.search(&g, "correr", &range, 10).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].id, jb.id);
    let page = Filter {
        page: Some(pb.page_id),
        ..Default::default()
    };
    assert_eq!(s.search(&g, "correr", &page, 10).unwrap()[0].id, pb.id);
    let pages = Filter {
        scope: Scope::Pages,
        ..Default::default()
    };
    assert!(s.search(&g, "correr", &pages, 10).unwrap().is_empty());
    assert_eq!(
        s.search(&g, &journal_title(j.journal_date.unwrap())[..4], &pages, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn vectors_pending_store_and_hybrid() {
    let (_d, g) = setup();
    let rust = g.ensure_page("Rust", None).unwrap();
    let cook = g.ensure_page("Cocina", None).unwrap();
    let b1 = add(
        &g,
        "Notes",
        "borrow checker lifetimes and ownership in rust code",
    );
    let b2 = add(&g, "Notes", "receta de cocina con arroz y verduras");
    let mut s = Search::open(&g).unwrap();
    s.sync(&g).unwrap();
    let emb = HashEmbedder::new(256);
    let model = "test/hash";

    let pending = s.pending_embeddings(&g, model, 100).unwrap();
    assert!(pending.len() >= 4);
    let n = s.embed_pending(&g, &emb, 3, 1000).unwrap();
    assert_eq!(n, pending.len());
    assert!(s.pending_embeddings(&g, model, 100).unwrap().is_empty());
    assert_eq!(s.vector_counts(&g, model).unwrap().1, 2);

    // Persisted and reloaded by a fresh Search.
    drop(s);
    let mut s = Search::open(&g).unwrap();
    s.sync(&g).unwrap();
    assert_eq!(s.vector_counts(&g, model).unwrap().1, 2);

    // Pure semantic hit with no shared full-text token (hash embedder shares words only).
    let qv = emb.embed_one("rust ownership");
    let v = s
        .vector_search(
            &g,
            model,
            &qv,
            &Filter {
                scope: Scope::Blocks,
                ..Default::default()
            },
            3,
        )
        .unwrap();
    assert_eq!(v[0].id, b1.id);

    // Hybrid: full-text finds "arroz", vectors find the rust block; both in results.
    let h = s
        .hybrid_search(&g, "arroz rust", Some(&emb), &Filter::default(), 5)
        .unwrap();
    let ids: Vec<_> = h.iter().map(|x| x.id).collect();
    assert!(ids.contains(&b1.id) && ids.contains(&b2.id));

    // Editing a block makes it pending again; deleting drops its vector.
    g.update_block(b2.id, "cambio de tema total").unwrap();
    assert_eq!(s.pending_embeddings(&g, model, 100).unwrap().len(), 1);
    g.delete_block(b1.id).unwrap();
    s.sync(&g).unwrap();
    assert_eq!(s.vector_counts(&g, model).unwrap().1, 1);

    // Semantic suggestion toward the Rust page.
    let b3 = add(&g, "Notes", "learning rust ownership");
    s.sync(&g).unwrap();
    s.embed_pending(&g, &emb, 10, 100).unwrap();
    let sg = s.semantic_suggestions(&g, model, b3.id, 3, 0.05).unwrap();
    assert_eq!(sg[0].target_page, rust.id);
    assert!(matches!(sg[0].reason, Reason::Semantic { .. }));
    assert!(
        sg.iter()
            .all(|x| x.target_page != cook.id || x.score < sg[0].score)
    );
    let applied = apply_suggestion(&g, &sg[0]).unwrap();
    assert_eq!(applied.content, "learning rust ownership [[Rust]]");
    assert!(
        s.semantic_suggestions(&g, model, b3.id, 3, 0.05)
            .unwrap()
            .iter()
            .all(|x| x.target_page != rust.id)
    );
    assert_eq!(s.related_pages(&g, model, rust.id, 3).unwrap().len(), 2);
    let _ = (cook,);
}

#[test]
fn unlinked_references_and_apply() {
    let (_d, g) = setup();
    let rust = g.create_page(NewPage::titled("Rust Lang")).unwrap();
    g.set_alias("rustlang", Some(rust.id)).unwrap();
    let b1 = add(
        &g,
        "Journal",
        "Today I read about rust lang and its borrow checker",
    );
    let b2 = add(
        &g,
        "Journal",
        "Already linked: [[Rust Lang]] and `rust lang` in code",
    );
    let b3 = add(&g, "Journal", "Ping RustLang folks");
    let mut s = Search::open(&g).unwrap();
    s.sync(&g).unwrap();

    let m = LinkMatcher::new(&g).unwrap();
    assert!(
        m.suggest_block(&g.get_block(b2.id).unwrap().unwrap(), true)
            .is_empty()
    );
    let sug = m.suggest_block(&g.get_block(b1.id).unwrap().unwrap(), true);
    assert_eq!(sug.len(), 1);
    assert_eq!(sug[0].reason, Reason::ExactTitle);

    let un = s.unlinked_references(&g, rust.id, 10).unwrap();
    let blocks: Vec<_> = un.iter().map(|x| x.block).collect();
    assert!(blocks.contains(&b1.id) && blocks.contains(&b3.id) && !blocks.contains(&b2.id));
    let alias = un.iter().find(|x| x.block == b3.id).unwrap();
    assert_eq!(alias.reason, Reason::Alias);

    let nb = apply_suggestion(&g, &sug[0]).unwrap();
    assert_eq!(
        nb.content,
        "Today I read about [[Rust Lang]] and its borrow checker"
    );
    // The link is now registered as a backlink and the suggestion is gone.
    assert_eq!(g.backlinks(rust.id).unwrap().len(), 2);
    assert!(m.suggest_block(&nb, true).is_empty());
    // Applying again fails: the text changed.
    assert!(matches!(
        apply_suggestion(&g, &sug[0]),
        Err(synapse_search::Error::Stale(_))
    ));
    assert_eq!(
        m.suggest_page(&g, g.page_by_title("Journal").unwrap().unwrap().id)
            .unwrap()
            .len(),
        1
    );
}
