//! tantivy-backed full-text index: one document per block and one per page (title + aliases,
//! plus the description of hubs as content).

use crate::error::Result;
use crate::{Filter, JournalFilter, Scope};
use chrono::{Local, NaiveDate};
use std::collections::BTreeSet;
use std::ops::Bound;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::{
    BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhraseQuery, Query, RangeQuery, TermQuery,
};
use tantivy::schema::{
    Field, IndexRecordOption, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{
    AsciiFoldingFilter, Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer,
    TextAnalyzer,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use uuid::Uuid;

const TOKENIZER: &str = "synapse";

/// Optional stemming applied at index and query time (changing it triggers a rebuild).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stemming {
    #[default]
    None,
    English,
    Spanish,
}

impl Stemming {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Stemming::None => "none",
            Stemming::English => "en",
            Stemming::Spanish => "es",
        }
    }
}

fn analyzer(stemming: Stemming) -> TextAnalyzer {
    let b = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(48))
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter);
    match stemming {
        Stemming::None => b.build(),
        Stemming::English => b.filter(Stemmer::new(Language::English)).build(),
        Stemming::Spanish => b.filter(Stemmer::new(Language::Spanish)).build(),
    }
}

#[derive(Clone, Copy)]
struct Fields {
    kind: Field,
    id: Field,
    page_id: Field,
    content: Field,
    title: Field,
    day: Field,
    journal: Field,
}

pub(crate) struct FullText {
    index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    f: Fields,
    analyzer: TextAnalyzer,
}

pub(crate) struct RawHit {
    pub is_page: bool,
    pub id: Uuid,
    pub page_id: Uuid,
    pub score: f32,
}

pub(crate) fn day_of_ms(ms: i64) -> i64 {
    let d = chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&Local).date_naive())
        .unwrap_or_default();
    day_of_date(d)
}

pub(crate) fn day_of_date(d: NaiveDate) -> i64 {
    use chrono::Datelike;
    d.num_days_from_ce() as i64 - 719_163
}

impl FullText {
    pub fn open(dir: &Path, stemming: Stemming, heap_bytes: usize) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut sb = Schema::builder();
        let text = |positions: bool| {
            TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(TOKENIZER)
                    .set_index_option(if positions {
                        IndexRecordOption::WithFreqsAndPositions
                    } else {
                        IndexRecordOption::WithFreqs
                    }),
            )
        };
        let f = Fields {
            kind: sb.add_text_field("kind", STRING | tantivy::schema::STORED),
            id: sb.add_text_field("id", STRING | tantivy::schema::STORED),
            page_id: sb.add_text_field("page_id", STRING | tantivy::schema::STORED),
            content: sb.add_text_field("content", text(true)),
            title: sb.add_text_field("title", text(true)),
            day: sb.add_i64_field(
                "day",
                tantivy::schema::INDEXED | tantivy::schema::FAST | tantivy::schema::STORED,
            ),
            journal: sb.add_u64_field(
                "journal",
                tantivy::schema::INDEXED | tantivy::schema::FAST | tantivy::schema::STORED,
            ),
        };
        let index =
            Index::open_or_create(tantivy::directory::MmapDirectory::open(dir)?, sb.build())?;
        let analyzer = analyzer(stemming);
        index.tokenizers().register(TOKENIZER, analyzer.clone());
        let writer = index.writer(heap_bytes.max(15_000_000))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            writer,
            f,
            analyzer,
        })
    }

    pub fn clear(&mut self) -> Result<()> {
        self.writer.delete_all_documents()?;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<()> {
        self.writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn doc_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    pub fn delete(&mut self, id: Uuid) {
        self.writer
            .delete_term(Term::from_field_text(self.f.id, &id.to_string()));
    }

    pub fn upsert_block(
        &mut self,
        id: Uuid,
        page_id: Uuid,
        content: &str,
        day: i64,
        journal: bool,
    ) -> Result<()> {
        self.delete(id);
        let mut d = TantivyDocument::default();
        d.add_text(self.f.kind, "b");
        d.add_text(self.f.id, id.to_string());
        d.add_text(self.f.page_id, page_id.to_string());
        d.add_text(self.f.content, content);
        d.add_i64(self.f.day, day);
        d.add_u64(self.f.journal, journal as u64);
        self.writer.add_document(d)?;
        Ok(())
    }

    pub fn upsert_page(
        &mut self,
        id: Uuid,
        title: &str,
        aliases: &[String],
        description: &str,
        day: i64,
        journal: bool,
    ) -> Result<()> {
        self.delete(id);
        let mut d = TantivyDocument::default();
        d.add_text(self.f.kind, "p");
        d.add_text(self.f.id, id.to_string());
        d.add_text(self.f.page_id, id.to_string());
        d.add_text(self.f.title, title);
        for a in aliases {
            d.add_text(self.f.title, a);
        }
        if !description.is_empty() {
            d.add_text(self.f.content, description);
        }
        d.add_i64(self.f.day, day);
        d.add_u64(self.f.journal, journal as u64);
        self.writer.add_document(d)?;
        Ok(())
    }

    pub fn tokens(&self, text: &str) -> Vec<String> {
        let mut a = self.analyzer.clone();
        let mut out = Vec::new();
        let mut s = a.token_stream(text);
        s.process(&mut |t| out.push(t.text.clone()));
        out
    }

    fn term_query(&self, field: Field, tok: &str, last: bool) -> Box<dyn Query> {
        let term = Term::from_field_text(field, tok);
        let n = tok.chars().count();
        if last {
            // Search-as-you-type: prefix match, tolerating one typo from 5 characters on.
            Box::new(FuzzyTermQuery::new_prefix(term, (n >= 5) as u8, true))
        } else if n >= 5 {
            Box::new(BooleanQuery::new(vec![
                (
                    Occur::Should,
                    Box::new(TermQuery::new(term.clone(), IndexRecordOption::WithFreqs))
                        as Box<dyn Query>,
                ),
                (
                    Occur::Should,
                    Box::new(BoostQuery::new(
                        Box::new(FuzzyTermQuery::new(term, 1, true)),
                        0.4,
                    )),
                ),
            ]))
        } else {
            Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs))
        }
    }

    fn text_query(&self, fields: &[(Field, f32)], toks: &[String], all: bool) -> Box<dyn Query> {
        let occur = if all { Occur::Must } else { Occur::Should };
        let clauses = toks
            .iter()
            .enumerate()
            .map(|(i, tok)| {
                let per_field = fields
                    .iter()
                    .map(|&(f, boost)| {
                        let q = self.term_query(f, tok, i + 1 == toks.len());
                        (
                            Occur::Should,
                            Box::new(BoostQuery::new(q, boost)) as Box<dyn Query>,
                        )
                    })
                    .collect();
                (
                    occur,
                    Box::new(BooleanQuery::new(per_field)) as Box<dyn Query>,
                )
            })
            .collect();
        Box::new(BooleanQuery::new(clauses))
    }

    fn filter_clauses(&self, filter: &Filter) -> Vec<(Occur, Box<dyn Query>)> {
        let mut c: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let term = |f: Field, v: &str| -> Box<dyn Query> {
            Box::new(TermQuery::new(
                Term::from_field_text(f, v),
                IndexRecordOption::Basic,
            ))
        };
        match filter.scope {
            Scope::All => {}
            Scope::Pages => c.push((Occur::Must, term(self.f.kind, "p"))),
            Scope::Blocks => c.push((Occur::Must, term(self.f.kind, "b"))),
        }
        match filter.journals {
            JournalFilter::Any => {}
            JournalFilter::Only => c.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(self.f.journal, 1),
                    IndexRecordOption::Basic,
                )),
            )),
            JournalFilter::Exclude => c.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(self.f.journal, 0),
                    IndexRecordOption::Basic,
                )),
            )),
        }
        if let Some(p) = filter.page {
            c.push((Occur::Must, term(self.f.page_id, &p.to_string())));
        }
        if filter.from.is_some() || filter.to.is_some() {
            let b = |d: Option<NaiveDate>| match d {
                Some(d) => Bound::Included(Term::from_field_i64(self.f.day, day_of_date(d))),
                None => Bound::Unbounded,
            };
            c.push((
                Occur::Must,
                Box::new(RangeQuery::new(b(filter.from), b(filter.to))),
            ));
        }
        c
    }

    fn run(&self, q: Box<dyn Query>, limit: usize) -> Result<Vec<RawHit>> {
        let searcher = self.reader.searcher();
        let top = searcher.search(&q, &TopDocs::with_limit(limit.max(1)))?;
        let mut out = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let d: TantivyDocument = searcher.doc(addr)?;
            let get = |f: Field| {
                d.get_first(f)
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
            };
            let (Some(id), Some(page_id)) = (get(self.f.id), get(self.f.page_id)) else {
                continue;
            };
            let is_page = d.get_first(self.f.kind).and_then(|v| v.as_str()) == Some("p");
            out.push(RawHit {
                is_page,
                id,
                page_id,
                score,
            });
        }
        Ok(out)
    }

    /// Ranked full-text search over blocks and page titles. All terms must match; when that
    /// finds nothing the query is relaxed to "any term".
    pub fn search(&self, text: &str, filter: &Filter, limit: usize) -> Result<Vec<RawHit>> {
        let toks = self.tokens(text);
        if toks.is_empty() {
            return Ok(Vec::new());
        }
        let fields = [(self.f.content, 1.0), (self.f.title, 3.0)];
        for all in [true, false] {
            let mut clauses = self.filter_clauses(filter);
            clauses.push((Occur::Must, self.text_query(&fields, &toks, all)));
            let hits = self.run(Box::new(BooleanQuery::new(clauses)), limit)?;
            if !hits.is_empty() || toks.len() == 1 {
                return Ok(hits);
            }
        }
        Ok(Vec::new())
    }

    /// Quick switcher: page titles and aliases only, prefix + typo tolerant.
    pub fn quick_switch(&self, text: &str, limit: usize) -> Result<Vec<RawHit>> {
        let toks = self.tokens(text);
        if toks.is_empty() {
            return Ok(Vec::new());
        }
        let clauses = vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.f.kind, "p"),
                    IndexRecordOption::Basic,
                )) as Box<dyn Query>,
            ),
            (
                Occur::Must,
                self.text_query(&[(self.f.title, 1.0)], &toks, true),
            ),
        ];
        self.run(Box::new(BooleanQuery::new(clauses)), limit)
    }

    /// Blocks whose text contains the phrase `text` (candidates for unlinked references).
    pub fn phrase_blocks(&self, text: &str, limit: usize) -> Result<Vec<RawHit>> {
        let toks = self.tokens(text);
        let q: Box<dyn Query> = match toks.len() {
            0 => return Ok(Vec::new()),
            1 => Box::new(TermQuery::new(
                Term::from_field_text(self.f.content, &toks[0]),
                IndexRecordOption::WithFreqs,
            )),
            _ => Box::new(PhraseQuery::new(
                toks.iter()
                    .map(|t| Term::from_field_text(self.f.content, t))
                    .collect(),
            )),
        };
        let clauses = vec![
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.f.kind, "b"),
                    IndexRecordOption::Basic,
                )) as Box<dyn Query>,
            ),
            (Occur::Must, q),
        ];
        self.run(Box::new(BooleanQuery::new(clauses)), limit)
    }

    /// Test/diagnostic helper: ids of all indexed docs.
    #[allow(dead_code)]
    pub fn all_ids(&self) -> Result<BTreeSet<Uuid>> {
        let searcher = self.reader.searcher();
        let top = searcher.search(
            &tantivy::query::AllQuery,
            &TopDocs::with_limit(searcher.num_docs() as usize + 1),
        )?;
        let mut out = BTreeSet::new();
        for (_, addr) in top {
            let d: TantivyDocument = searcher.doc(addr)?;
            if let Some(id) = d
                .get_first(self.f.id)
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                out.insert(id);
            }
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn index(&self) -> &Index {
        &self.index
    }
}
