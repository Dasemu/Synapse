use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::NaiveDate;
use synapse_ai::features::*;
use synapse_ai::*;
use synapse_core::{Graph, NewPage, Placement};

/// Chat provider that returns a canned reply and records the request.
struct Scripted {
    reply: String,
    seen: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl ChatProvider for Scripted {
    async fn chat(
        &self,
        req: &ChatRequest,
        sink: Option<TokenSink<'_>>,
        _: &CancellationToken,
    ) -> Result<ChatResponse> {
        self.seen.lock().unwrap().push(req.clone());
        if let Some(s) = sink {
            s(&self.reply);
        }
        Ok(ChatResponse {
            text: self.reply.clone(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: None,
        })
    }
}

fn client(reply: &str) -> (AiClient, Arc<Scripted>) {
    let p = Arc::new(Scripted {
        reply: reply.into(),
        seen: Mutex::default(),
    });
    (
        AiClient::from_providers(ProviderConfig::default(), p.clone(), None),
        p,
    )
}

fn graph() -> (tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    let g = Graph::open(dir.path()).unwrap();
    (dir, g)
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

#[tokio::test]
async fn ask_graph_builds_context_and_filters_citations() {
    let (_d, g) = graph();
    let page = g.create_page(NewPage::titled("Rust")).unwrap();
    let b1 = g
        .insert_block(
            page.id,
            Placement::LastChildOf(None),
            "Ownership prevents data races",
        )
        .unwrap();
    let b2 = g
        .insert_block(page.id, Placement::LastChildOf(None), "Unrelated")
        .unwrap();
    let bogus = uuid::Uuid::now_v7();
    let reply = format!("Rust prevents races (({})) and also (({bogus})).", b1.id);
    let (ai, p) = client(&reply);

    let ids = vec![b1.id, bogus, b1.id];
    let job = ask_graph(
        &g,
        "How does Rust avoid races?",
        &|_q| ids.clone(),
        &AskOptions::default(),
    )
    .unwrap();
    let mut streamed = String::new();
    let mut sink = |t: &str| streamed.push_str(t);
    let out = job
        .run(&ai, Some(&mut sink), &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(out.value.citations, vec![b1.id]);
    assert_eq!(out.value.context, vec![b1.id]);
    assert_eq!(out.usage.total(), 15);
    assert_eq!(streamed, reply);
    let sent = p.seen.lock().unwrap()[0].clone();
    let user = &sent.messages[0].content;
    assert!(user.contains(&format!("id=\"(({}))\"", b1.id)) && user.contains("page=\"Rust\""));
    assert!(!user.contains(&b2.id.to_string()));
    assert!(sent.system.unwrap().contains("same language"));
}

#[tokio::test]
async fn summarize_skips_the_model_when_journals_are_empty() {
    let (_d, g) = graph();
    let (ai, p) = client("never used");
    let out = summarize_journal(&g, d("2026-03-02"), d("2026-03-08"))
        .unwrap()
        .run(&ai, None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out.value, "");
    assert!(p.seen.lock().unwrap().is_empty());

    let j = g.journal(d("2026-03-03")).unwrap();
    g.insert_block(
        j.id,
        Placement::LastChildOf(None),
        "Met Ana about the launch",
    )
    .unwrap();
    let out = summarize_journal(&g, d("2026-03-02"), d("2026-03-08"))
        .unwrap()
        .run(&ai, None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(out.value, "never used");
    let user = p.seen.lock().unwrap()[0].messages[0].content.clone();
    assert!(user.contains("<journal date=\"2026-03-03\">") && user.contains("Met Ana"));
}

#[tokio::test]
async fn suggestions_are_validated_against_the_graph() {
    let (_d, g) = graph();
    let target = g.create_page(NewPage::titled("Launch Plan")).unwrap();
    let linked = g.create_page(NewPage::titled("Ana")).unwrap();
    let page = g.create_page(NewPage::titled("Meeting")).unwrap();
    let b = g
        .insert_block(
            page.id,
            Placement::LastChildOf(None),
            "Talked with [[Ana]] about the launch",
        )
        .unwrap();
    g.update_block(b.id, "Talked with [[Ana]] about the launch")
        .unwrap();
    let reply = format!(
        "```json\n{{\"links\":[{{\"target\":\"launch plan\",\"reason\":\"same topic\",\"block\":\"{}\"}},\
{{\"target\":\"Ana\",\"reason\":\"dup\"}},{{\"target\":\"Meeting\",\"reason\":\"self\"}},\
{{\"target\":\"Budget\",\"reason\":\"new\",\"block\":\"{}\"}}],\"tags\":[{{\"tag\":\"#work\",\"reason\":\"r\"}}]}}\n```",
        b.id,
        uuid::Uuid::now_v7()
    );
    let _ = (target, linked);
    let (ai, _) = client(&reply);
    let s = suggest_links_and_tags(&g, page.id)
        .unwrap()
        .run(&ai, None, &CancellationToken::new())
        .await
        .unwrap()
        .value;
    assert_eq!(s.links.len(), 2, "{s:?}");
    assert!(s.links[0].exists && s.links[0].block == Some(b.id));
    assert!(!s.links[1].exists && s.links[1].target == "Budget" && s.links[1].block.is_none());
    assert_eq!(s.tags[0].tag, "work");
}

#[tokio::test]
async fn extract_tasks_and_insert_todo_blocks() {
    let (_d, g) = graph();
    let page = g.create_page(NewPage::titled("Notes")).unwrap();
    let b = g
        .insert_block(
            page.id,
            Placement::LastChildOf(None),
            "Call the bank tomorrow",
        )
        .unwrap();
    let sibling = g
        .insert_block(page.id, Placement::LastChildOf(None), "SIBLING-SECRET")
        .unwrap();
    let (ai, p) = client(
        r#"{"tasks":[{"text":"Call the bank","due":"2026-03-04","priority":"A"},{"text":"Send invoice","due":"nonsense","priority":"Z"},{"text":" "}]}"#,
    );
    let tasks = extract_tasks(&g, b.id)
        .unwrap()
        .run(&ai, None, &CancellationToken::new())
        .await
        .unwrap()
        .value;
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        tasks[0].to_block_content(),
        "TODO [#A] Call the bank\nDEADLINE: <2026-03-04 Wed>"
    );
    assert_eq!((tasks[1].due, tasks[1].priority), (None, None));
    assert!(
        !p.seen.lock().unwrap()[0].messages[0]
            .content
            .contains("SIBLING-SECRET")
    );
    let _ = sibling;

    let made = insert_tasks(&g, b.id, &tasks).unwrap();
    assert_eq!(made.len(), 2);
    assert!(
        made.iter()
            .all(|m| m.parent_id == Some(b.id) && m.content.starts_with("TODO"))
    );
}

#[tokio::test]
async fn consolidate_week_proposal_and_apply() {
    let (_d, g) = graph();
    let (ai, _) = client("unused");
    assert!(
        consolidate_week(&g, d("2026-03-02"))
            .unwrap()
            .run(&ai, None, &CancellationToken::new())
            .await
            .unwrap()
            .value
            .is_none()
    );

    let j = g.journal(d("2026-03-04")).unwrap();
    let b = g
        .insert_block(
            j.id,
            Placement::LastChildOf(None),
            "Learned that small batches ship faster",
        )
        .unwrap();
    let reply = format!(
        "{{\"title\":\"Small batches\",\"blocks\":[\"Small batches ship faster.\"],\"tags\":[\"#process\"],\"sources\":[\"(({}))\",\"{}\"]}}",
        b.id,
        uuid::Uuid::now_v7()
    );
    let (ai, _) = client(&reply);
    let prop = consolidate_week(&g, d("2026-03-02"))
        .unwrap()
        .run(&ai, None, &CancellationToken::new())
        .await
        .unwrap()
        .value
        .unwrap();
    assert_eq!(prop.sources, vec![b.id]);
    assert_eq!(prop.tags, ["process"]);

    let id = apply_proposal(&g, &prop).unwrap();
    let tree = g.page_tree(id).unwrap();
    let texts: Vec<_> = tree.iter().map(|(_, b)| b.content.clone()).collect();
    assert_eq!(texts[0], "Small batches ship faster.");
    assert!(texts.contains(&"tags:: [[process]]".to_string()));
    assert!(texts.contains(&format!("(({}))", b.id)));
}

#[tokio::test]
async fn bad_model_json_is_a_typed_error() {
    let (_d, g) = graph();
    let page = g.create_page(NewPage::titled("P")).unwrap();
    let (ai, _) = client("I cannot do that");
    let e = suggest_links_and_tags(&g, page.id)
        .unwrap()
        .run(&ai, None, &CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert!(matches!(e, Error::BadModelOutput(_)));
}

#[test]
fn block_refs_parse() {
    let a = uuid::Uuid::now_v7();
    assert_eq!(
        parse_block_refs(&format!("x (({a})) y (({a})) ((not-a-uuid))")),
        vec![a]
    );
}
