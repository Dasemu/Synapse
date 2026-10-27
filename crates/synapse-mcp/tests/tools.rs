//! Every tool on a temp graph, through the same read/write wrappers the server uses.

use chrono::NaiveDate;
use synapse_core::{Graph, NewPage, Placement};
use synapse_mcp::params::*;
use synapse_mcp::tools::{self, ToolResult};
use synapse_mcp::{read_job, write_job};

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// 2026-03-10 is "today". Journals on 03-08 and 03-10; a typed+tagged book in hub "Reading".
fn fixture() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    let noon = |d: &str| {
        day(d)
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .unwrap()
            .timestamp_millis()
    };
    g.set_now(Some(noon("2026-03-08")));
    let j1 = g.journal(day("2026-03-08")).unwrap();
    let b = g
        .insert_block(
            j1.id,
            Placement::LastChildOf(None),
            "Started reading [[Dune]] by Frank Herbert",
        )
        .unwrap();
    g.insert_block(
        j1.id,
        Placement::LastChildOf(Some(b.id)),
        "the canción of spice",
    )
    .unwrap();
    let hub = g.create_hub("Reading", "Books and articles").unwrap();
    let dune = g.page_by_title("Dune").unwrap().unwrap();
    g.set_page_type(dune.id, Some("Book")).unwrap();
    g.set_page_tags(dune.id, vec!["scifi".into()]).unwrap();
    g.add_to_hub(dune.id, hub.id).unwrap();
    g.insert_block(
        dune.id,
        Placement::LastChildOf(None),
        "Arrakis is a desert planet",
    )
    .unwrap();
    g.set_now(Some(noon("2026-03-10")));
    let j2 = g.journal(day("2026-03-10")).unwrap();
    g.insert_block(
        j2.id,
        Placement::LastChildOf(None),
        "Talked about Dune and [[Gardening]] with Ana",
    )
    .unwrap();
    (dir, g)
}

fn read(g: &Graph, f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static) -> String {
    read_job(f)(g).unwrap_or_else(|e| panic!("tool failed: {e}"))
}

fn read_err(g: &Graph, f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static) -> String {
    read_job(f)(g).expect_err("expected an error").message
}

fn write(g: &Graph, f: impl Fn(&Graph) -> ToolResult + Send + Sync + 'static) -> ToolResult {
    write_job(f)(g)
}

#[test]
fn search_finds_blocks_and_titles_with_filters() {
    let (_d, g) = fixture();
    let out = read(&g, |g| {
        tools::search(
            g,
            &SearchParams {
                query: "dune".into(),
                ..Default::default()
            },
        )
    });
    assert!(out.starts_with("3 results"), "{out}");
    assert!(
        out.lines().nth(1).unwrap().contains("[[Dune]] (page"),
        "{out}"
    );
    assert!(out.contains("[[2026-03-08]] (("), "{out}");
    // Accent-insensitive, journals only, date filter.
    let out = read(&g, |g| {
        tools::search(
            g,
            &SearchParams {
                query: "cancion".into(),
                journals: Some("only".into()),
                from: Some("2026-03-08".into()),
                to: Some("2026-03-08".into()),
                ..Default::default()
            },
        )
    });
    assert!(out.contains("canción of spice"), "{out}");
    // Tag filter keeps only the tagged page's content.
    let out = read(&g, |g| {
        tools::search(
            g,
            &SearchParams {
                query: "arrakis".into(),
                tag: Some("scifi".into()),
                ..Default::default()
            },
        )
    });
    assert!(out.starts_with("1 results"), "{out}");
    let out = read(&g, |g| {
        tools::search(
            g,
            &SearchParams {
                query: "dune".into(),
                limit: Some(1),
                ..Default::default()
            },
        )
    });
    assert!(out.contains("next cursor: 1"), "{out}");
    assert!(read_err(&g, |g| tools::search(g, &SearchParams::default())).contains("empty"));
}

#[test]
fn get_page_renders_header_and_outline() {
    let (_d, g) = fixture();
    let out = read(&g, |g| {
        tools::get_page(
            g,
            &GetPageParams {
                page: "dune".into(),
                ..Default::default()
            },
        )
    });
    assert!(out.starts_with("# Dune\n"), "{out}");
    assert!(out.contains("- type: Book"), "{out}");
    assert!(out.contains("- tags: scifi"), "{out}");
    assert!(out.contains("- hub: Reading"), "{out}");
    assert!(out.contains("born in [[2026-03-08]]"), "{out}");
    assert!(out.contains("- Arrakis is a desert planet (("), "{out}");
    // Journal by date, subtree and depth, no ids.
    let out = read(&g, |g| {
        tools::get_page(
            g,
            &GetPageParams {
                page: "2026-03-08".into(),
                max_depth: Some(0),
                ids: Some(false),
                ..Default::default()
            },
        )
    });
    assert!(
        out.contains("- Started reading [[Dune]] by Frank Herbert\n"),
        "{out}"
    );
    assert!(!out.contains("spice"), "{out}");
    // Hub lists members.
    let out = read(&g, |g| {
        tools::get_page(
            g,
            &GetPageParams {
                page: "Reading".into(),
                ..Default::default()
            },
        )
    });
    assert!(out.contains("## Members (1)\n- [[Dune]]"), "{out}");
    let e = read_err(&g, |g| {
        tools::get_page(
            g,
            &GetPageParams {
                page: "Dun".into(),
                ..Default::default()
            },
        )
    });
    assert!(e.contains("did you mean: [[Dune]]"), "{e}");
}

#[test]
fn get_journal_day_and_range_never_create() {
    let (_d, g) = fixture();
    let seq = g.last_seq().unwrap();
    let out = read(&g, |g| {
        tools::get_journal(
            g,
            &GetJournalParams {
                date: Some("today".into()),
                ..Default::default()
            },
        )
    });
    assert!(out.starts_with("# 2026-03-10\n- Talked about"), "{out}");
    let out = read(&g, |g| {
        tools::get_journal(
            g,
            &GetJournalParams {
                date: Some("2026-03-09".into()),
                ..Default::default()
            },
        )
    });
    assert!(out.contains("(no journal)"), "{out}");
    let out = read(&g, |g| {
        tools::get_journal(
            g,
            &GetJournalParams {
                from: Some("2026-03-01".into()),
                ..Default::default()
            },
        )
    });
    let a = out.find("# 2026-03-08").unwrap();
    let b = out.find("# 2026-03-10").unwrap();
    assert!(a < b, "oldest first: {out}");
    assert_eq!(g.last_seq().unwrap(), seq, "reads must not write");
    assert!(
        g.get_page(synapse_core::store::journal_id(day("2026-03-09")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn list_pages_filters_and_sorts() {
    let (_d, g) = fixture();
    let out = read(&g, |g| tools::list_pages(g, &ListPagesParams::default()));
    assert!(
        out.contains("- [[Dune]] · type: Book · tags: scifi · hub: Reading"),
        "{out}"
    );
    assert!(!out.contains("[[2026-03-08]]"), "journals excluded: {out}");
    let out = read(&g, |g| {
        tools::list_pages(
            g,
            &ListPagesParams {
                untyped: Some(true),
                ..Default::default()
            },
        )
    });
    assert!(
        out.contains("[[Gardening]]") && !out.contains("[[Dune]]"),
        "{out}"
    );
    assert!(!out.contains("[[Reading]]"), "hubs excluded: {out}");
    let out = read(&g, |g| {
        tools::list_pages(
            g,
            &ListPagesParams {
                page_type: Some("book".into()),
                ..Default::default()
            },
        )
    });
    assert!(out.starts_with("1 pages"), "{out}");
    let out = read(&g, |g| {
        tools::list_pages(
            g,
            &ListPagesParams {
                journals: Some("only".into()),
                sort: Some("title".into()),
                ..Default::default()
            },
        )
    });
    assert!(
        out.find("2026-03-08").unwrap() < out.find("2026-03-10").unwrap(),
        "{out}"
    );
    let out = read(&g, |g| {
        tools::list_pages(
            g,
            &ListPagesParams {
                created_since: Some("2026-03-10".into()),
                ..Default::default()
            },
        )
    });
    assert!(
        out.contains("[[Gardening]]") && !out.contains("[[Dune]]"),
        "{out}"
    );
}

#[test]
fn backlinks_and_related() {
    let (_d, g) = fixture();
    let out = read(&g, |g| {
        tools::backlinks(
            g,
            &PageRefParams {
                page: "Dune".into(),
                ..Default::default()
            },
        )
    });
    assert!(
        out.starts_with("1 linked references to [[Dune]] from 1 pages"),
        "{out}"
    );
    assert!(
        out.contains("## [[2026-03-08]]\n- Started reading"),
        "{out}"
    );
    let out = read(&g, |g| {
        tools::related(
            g,
            &PageRefParams {
                page: "Dune".into(),
                ..Default::default()
            },
        )
    });
    assert!(out.contains("[[2026-03-08]]"), "{out}");
}

#[test]
fn tags_types_hubs_timeline() {
    let (_d, g) = fixture();
    let out = read(&g, |g| tools::list_tags(g, &ListParams::default()));
    assert!(out.contains("- scifi — 1 (1/0/0)"), "{out}");
    assert!(out.contains("- Book — 1 (0/1/0)"), "{out}");
    assert!(out.contains("- Reading — 1 (0/0/1)"), "{out}");
    let out = read(&g, |g| tools::list_types(g, &ListParams::default()));
    assert!(
        out.starts_with("1 types in use:\n- Book — 1 pages"),
        "{out}"
    );
    let out = read(&g, |g| {
        tools::hubs(
            g,
            &HubsParams {
                members: Some(true),
                ..Default::default()
            },
        )
    });
    assert!(
        out.contains("- [[Reading]] (1 notes) — Books and articles\n  - Dune"),
        "{out}"
    );
    let out = read(&g, |g| {
        tools::hubs(
            g,
            &HubsParams {
                hub: Some("Reading".into()),
                ..Default::default()
            },
        )
    });
    assert!(
        out.contains("- [[Dune]] · type: Book · tags: scifi"),
        "{out}"
    );
    let out = read(&g, |g| tools::timeline(g, &RangeParams::default()));
    assert!(out.contains("## 2026-03-08 (journal)\n- born: "), "{out}");
    assert!(out.contains("[[Dune]]"), "{out}");
    let e = read_err(&g, |g| {
        tools::timeline(
            g,
            &RangeParams {
                from: Some("2025-01-01".into()),
                to: None,
            },
        )
    });
    assert!(e.contains("range too long"), "{e}");
}

#[test]
fn journal_links_reports_status_and_unlinked_mentions() {
    let (_d, g) = fixture();
    let out = read(&g, |g| tools::journal_links(g, &RangeParams::default()));
    assert!(
        out.contains("## 2026-03-08\n- linked [[Dune]] · type: Book"),
        "{out}"
    );
    assert!(
        out.contains("- linked [[Gardening]] · untyped · untagged · empty"),
        "{out}"
    );
    assert!(out.contains("- mentioned (not linked) [[Dune]]"), "{out}");
}

#[test]
fn write_tools_curate_pages() {
    let (_d, g) = fixture();
    let out = write(&g, |g| {
        tools::create_page(
            g,
            &CreatePageParams {
                title: "Frank Herbert".into(),
                page_type: Some("Person".into()),
                tags: Some(vec!["author".into(), "scifi".into()]),
                hub: Some("Reading".into()),
                content: Some("- Wrote [[Dune]]\n  - 1965\n- born:: 1920".into()),
                source_journal: Some("2026-03-08".into()),
            },
        )
    })
    .unwrap();
    assert!(out.starts_with("Created [[Frank Herbert]]"), "{out}");
    assert!(out.contains("with 3 blocks"), "{out}");
    let p = g.page_by_title("frank herbert").unwrap().unwrap();
    assert_eq!(p.page_type(), Some("Person"));
    assert_eq!(p.tags, vec!["author", "scifi"]);
    assert_eq!(p.origin_journal, Some(day("2026-03-08")));
    let tree = g.page_tree(p.id).unwrap();
    assert_eq!(tree[1].0, 1, "nested child");
    // Duplicate title.
    let e = write(&g, |g| {
        tools::create_page(
            g,
            &CreatePageParams {
                title: "Dune".into(),
                ..Default::default()
            },
        )
    })
    .unwrap_err();
    assert!(e.message.contains("already exists"), "{e}");

    let out = write(&g, |g| {
        tools::append_blocks(
            g,
            &AppendBlocksParams {
                page: "Frank Herbert".into(),
                content: "- Also wrote [[Children of Dune]]".into(),
                parent: None,
            },
        )
    })
    .unwrap();
    assert!(out.starts_with("Appended 1 blocks"), "{out}");
    let first = g.page_tree(p.id).unwrap()[0].1.id;
    let out = write(&g, move |g| {
        tools::update_block(
            g,
            &UpdateBlockParams {
                block: format!("(({first}))"),
                content: "Wrote [[Dune]] (1965)".into(),
            },
        )
    })
    .unwrap();
    assert!(out.contains("Wrote [[Dune]] (1965)"), "{out}");
    let out = write(&g, move |g| {
        tools::delete_block(
            g,
            &DeleteBlockParams {
                block: first.to_string(),
            },
        )
    })
    .unwrap();
    assert!(out.contains("and 1 children"), "{out}");

    write(&g, |g| {
        tools::set_page_type(
            g,
            &SetPageTypeParams {
                page: "Gardening".into(),
                page_type: Some("Topic".into()),
            },
        )
    })
    .unwrap();
    let out = write(&g, |g| {
        tools::set_page_tags(
            g,
            &SetPageTagsParams {
                page: "Gardening".into(),
                add: Some(vec!["hobby".into(), "outdoors".into()]),
                remove: Some(vec!["Outdoors".into()]),
                ..Default::default()
            },
        )
    })
    .unwrap();
    assert_eq!(out, "[[Gardening]] tags: hobby\n");
    let out = write(&g, |g| {
        tools::set_page_hub(
            g,
            &SetPageHubParams {
                page: "Gardening".into(),
                hub: Some("Reading".into()),
            },
        )
    })
    .unwrap();
    assert!(out.contains("now in hub [[Reading]]"), "{out}");
    write(&g, |g| {
        tools::set_page_hub(
            g,
            &SetPageHubParams {
                page: "Gardening".into(),
                hub: None,
            },
        )
    })
    .unwrap();
    let gp = g.page_by_title("Gardening").unwrap().unwrap();
    assert_eq!(gp.page_type(), Some("Topic"));
    assert!(gp.hubs.is_empty());
}

#[test]
fn write_tools_refuse_journals_and_roll_back_on_error() {
    let (_d, g) = fixture();
    let jblock = g
        .page_tree(synapse_core::store::journal_id(day("2026-03-10")))
        .unwrap()[0]
        .1
        .id;
    let seq = g.last_seq().unwrap();
    for e in [
        write(&g, |g| {
            tools::append_blocks(
                g,
                &AppendBlocksParams {
                    page: "today".into(),
                    content: "- x".into(),
                    parent: None,
                },
            )
        }),
        write(&g, move |g| {
            tools::update_block(
                g,
                &UpdateBlockParams {
                    block: jblock.to_string(),
                    content: "x".into(),
                },
            )
        }),
        write(&g, move |g| {
            tools::delete_block(
                g,
                &DeleteBlockParams {
                    block: jblock.to_string(),
                },
            )
        }),
        write(&g, |g| {
            tools::set_page_tags(
                g,
                &SetPageTagsParams {
                    page: "2026-03-08".into(),
                    tags: Some(vec!["x".into()]),
                    ..Default::default()
                },
            )
        }),
        write(&g, |g| {
            tools::create_page(
                g,
                &CreatePageParams {
                    title: "2026-04-01".into(),
                    ..Default::default()
                },
            )
        }),
        // Fails after creating the page (unknown hub is checked first; bad tag is not):
        // the whole call must roll back.
        write(&g, |g| {
            tools::create_page(
                g,
                &CreatePageParams {
                    title: "Half done".into(),
                    tags: Some(vec!["x".repeat(500)]),
                    ..Default::default()
                },
            )
        }),
        write(&g, |g| {
            tools::create_page(
                g,
                &CreatePageParams {
                    title: "From missing day".into(),
                    source_journal: Some("2026-03-09".into()),
                    ..Default::default()
                },
            )
        }),
    ] {
        assert!(e.is_err(), "{e:?}");
    }
    assert_eq!(g.last_seq().unwrap(), seq, "nothing written");
    assert!(g.page_by_title("Half done").unwrap().is_none());
}

#[test]
fn reads_are_rolled_back() {
    let (_d, g) = fixture();
    let seq = g.last_seq().unwrap();
    // A read job that tries to write is undone.
    let out = read(&g, |g| {
        g.create_page(NewPage::titled("Sneaky"))?;
        Ok("ok".into())
    });
    assert_eq!(out, "ok");
    assert_eq!(g.last_seq().unwrap(), seq);
    assert!(g.page_by_title("Sneaky").unwrap().is_none());
}

#[test]
fn resources_render_pages() {
    let (_d, g) = fixture();
    let out = read(&g, |g| tools::page_resource(g, "Dune"));
    assert!(out.contains("- Arrakis is a desert planet\n"), "{out}");
}

#[test]
fn extreme_dates_and_cursors_are_rejected() {
    let (_d, g) = fixture();
    let e = read_err(&g, |g| {
        tools::timeline(
            g,
            &RangeParams {
                from: None,
                to: Some("-262143-01-03".into()),
            },
        )
    });
    assert!(e.contains("bad date"), "{e}");
    let e = read_err(&g, |g| {
        tools::list_pages(
            g,
            &ListPagesParams {
                cursor: Some(usize::MAX.to_string()),
                ..Default::default()
            },
        )
    });
    assert!(e.contains("bad cursor"), "{e}");
}
