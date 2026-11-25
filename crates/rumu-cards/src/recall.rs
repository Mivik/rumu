//! BM25F ranking over an inverted [`Index`]: [`rank_segments`] scores the
//! documents of several indexes as one corpus, and [`rank_docs`] is the
//! one-off form over a list of [`Doc`]s. Card recall keeps one index per deck
//! (see [`crate::cache`]) and ranks the mounted decks' together.
//!
//! A card's frontmatter `keywords:` (weight 3) outweigh its path (2), its
//! description (1.5) and its body (1), so a card that names its own trigger
//! terms is found even when its prose never repeats them. Text is tokenized
//! into lowercase words in any alphabet and CJK bigrams (fullwidth ASCII
//! folds to halfwidth); an `_`-joined word also emits its 2+-char parts, so
//! `session_search` matches `session` and `search`. Documents additionally
//! index a CJK run's single chars as overlap terms, so a one-char search
//! query hits; those count toward neither field length nor recall — a reply
//! like `好` must not recall every card containing the char.
//!
//! A card only qualifies when it matches a term rare enough to say something:
//! from [`FLOOR_MIN_CARDS`] cards on, its score must reach [`FLOOR`] times
//! the IDF of a term appearing in a single card. Below that the corpus is too
//! small for IDF to be meaningful, so there is no floor. Explicit search
//! ranks with `recall = false`: any positive score is a hit, and a term common
//! to every document still matches.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::Card;

/// The most fields a document has.
pub const FIELDS: usize = 4;
/// Per-field card weights: keywords, path, description, body.
pub const CARD_WEIGHTS: [f64; FIELDS] = [3.0, 2.0, 1.5, 1.0];
/// The path field of a card document.
pub const PATH_FIELD: usize = 1;
/// BM25 term-frequency saturation and length normalization.
const K1: f64 = 1.2;
const B: f64 = 0.75;
/// Below this many cards there is no relevance floor (IDF is too noisy).
const FLOOR_MIN_CARDS: u64 = 16;
/// The relevance floor, as a fraction of the IDF of a term that appears in
/// exactly one card.
const FLOOR: f64 = 0.7;

/// Counts per field.
type Tf = [u32; FIELDS];

/// One tokenized document: each term's count per field, each field's token
/// length, and a score multiplier.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    terms: BTreeMap<String, Tf>,
    lens: Tf,
    weights: [f64; FIELDS],
    boost: f64,
}

impl Doc {
    /// Tokenize `(text, weight)` fields (at most [`FIELDS`]); `boost`
    /// multiplies the final score. All documents of one ranking list the
    /// same fields in one order.
    pub fn new<'t>(fields: impl IntoIterator<Item = (&'t str, f64)>, boost: f64) -> Self {
        let mut doc = Self {
            boost,
            ..Self::default()
        };
        for (f, (text, weight)) in fields.into_iter().enumerate() {
            assert!(f < FIELDS, "a document has at most {FIELDS} fields");
            doc.weights[f] = weight;
            for (t, overlap) in scan(text, true) {
                doc.terms.entry(t).or_default()[f] += 1;
                if !overlap {
                    doc.lens[f] += 1;
                }
            }
        }
        doc
    }

    /// A card's document: its keywords, `path`, summary and body.
    pub fn card(card: &Card, path: &str) -> Self {
        let kw = card.keywords().join(" ");
        let desc = card.summary().unwrap_or_default();
        let fields = [kw.as_str(), path, desc.as_str(), card.body.as_str()];
        Self::new(fields.into_iter().zip(CARD_WEIGHTS), 1.0)
    }
}

/// An inverted index over documents with one field layout.
#[derive(Debug, Default)]
pub struct Index {
    weights: [f64; FIELDS],
    /// Term → the documents that have it, by document, with its counts.
    postings: HashMap<Box<str>, Box<[(u32, Tf)]>>,
    lens: Vec<Tf>,
    boosts: Vec<f64>,
}

impl Index {
    /// Index `docs` (document `i` is the `i`th) under per-field `weights`.
    pub fn new<'d>(weights: [f64; FIELDS], docs: impl IntoIterator<Item = &'d Doc>) -> Self {
        let docs: Vec<&Doc> = docs.into_iter().collect();
        let empty = Self {
            weights,
            ..Self::default()
        };
        empty.update(&[], docs.len(), (0..).zip(docs))
    }

    /// This index with its documents renumbered by `kept` (old document →
    /// its new number, `None` when dropped) and the `added` ones (new
    /// number, document) indexed in: `len` documents in all, each either
    /// kept or added. Kept documents are not tokenized again.
    pub fn update<'d>(
        &self,
        kept: &[Option<u32>],
        len: usize,
        added: impl IntoIterator<Item = (u32, &'d Doc)>,
    ) -> Self {
        let mut lens = vec![Tf::default(); len];
        let mut boosts = vec![1.0; len];
        for (old, new) in kept.iter().enumerate() {
            if let Some(new) = *new {
                lens[new as usize] = self.lens[old];
                boosts[new as usize] = self.boosts[old];
            }
        }
        let mut postings: HashMap<&str, Vec<(u32, Tf)>> = self
            .postings
            .iter()
            .filter_map(|(t, list)| {
                let list: Vec<(u32, Tf)> = list
                    .iter()
                    .filter_map(|&(d, tf)| Some((kept[d as usize]?, tf)))
                    .collect();
                (!list.is_empty()).then_some((&**t, list))
            })
            .collect();
        for (d, doc) in added {
            lens[d as usize] = doc.lens;
            boosts[d as usize] = doc.boost;
            for (t, tf) in &doc.terms {
                postings.entry(t).or_default().push((d, *tf));
            }
        }
        // Boxed: the index lives on in the cache, without spare capacity.
        let postings = postings
            .into_iter()
            .map(|(t, mut list)| {
                if !list.is_sorted_by_key(|(d, _)| *d) {
                    list.sort_unstable_by_key(|(d, _)| *d);
                }
                (t.into(), list.into_boxed_slice())
            })
            .collect();
        Self {
            weights: self.weights,
            postings,
            lens,
            boosts,
        }
    }

    /// The number of documents.
    pub fn len(&self) -> usize {
        self.lens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lens.is_empty()
    }
}

/// Terms every document of a [`Segment`] has besides its own, in one field:
/// a card's mount point, which is part of its path but differs per session.
#[derive(Debug)]
pub struct Shared {
    field: usize,
    terms: BTreeMap<String, u32>,
    len: u32,
}

impl Shared {
    /// Tokenize `text` as part of `field`. Tokenizing `a/b` gives the terms
    /// of `a` then those of `b`, so a shared prefix splits off exactly.
    pub fn new(field: usize, text: &str) -> Self {
        let mut terms = BTreeMap::new();
        let mut len = 0;
        for (t, overlap) in scan(text, true) {
            *terms.entry(t).or_default() += 1;
            if !overlap {
                len += 1;
            }
        }
        Self { field, terms, len }
    }
}

/// The documents of one index that take part in a ranking.
pub struct Segment<'a> {
    pub index: &'a Index,
    /// Per document of `index`: its key in the ranking, or `None` when it
    /// does not take part. Equal scores rank by key.
    pub keys: Vec<Option<u32>>,
    pub shared: Option<Shared>,
}

impl Segment<'_> {
    /// How often every document here has `t` besides its own count.
    fn shared_tf(&self, t: &str) -> u32 {
        self.shared
            .as_ref()
            .and_then(|s| s.terms.get(t))
            .copied()
            .unwrap_or(0)
    }
}

/// Rank the documents of `segments` against `query` by BM25F as one corpus,
/// best first (ties by key), at most `limit`, as `(key, score)`. Document
/// frequencies, the document count and the average field lengths are summed
/// over the segments, so scores are those of one index over every document
/// taking part. `recall` applies the relevance floor and drops terms found
/// in every document.
pub fn rank_segments(
    segments: &[Segment<'_>],
    query: &str,
    limit: usize,
    recall: bool,
) -> Vec<(u32, f64)> {
    // Recall ignores one-char query terms, so a reply like `好` does not
    // recall every card containing the char; explicit search keeps them.
    let query: BTreeSet<String> = tokens(query)
        .into_iter()
        .filter(|t| !recall || t.chars().count() > 1)
        .collect();
    let counts: Vec<u64> = segments
        .iter()
        .map(|s| s.keys.iter().flatten().count() as u64)
        .collect();
    let n: u64 = counts.iter().sum();
    if query.is_empty() || n == 0 || limit == 0 {
        return Vec::new();
    }

    // Document frequency: a term counts once per document, whatever its field.
    // A term in every document says nothing about which one is relevant (a
    // single-document corpus is exempt: every term would be in "every" doc).
    let df = |t: &str| -> u64 {
        segments
            .iter()
            .zip(&counts)
            .map(|(s, &count)| {
                if s.shared_tf(t) > 0 {
                    return count;
                }
                s.index.postings.get(t).map_or(0, |p| {
                    p.iter()
                        .filter(|(d, _)| s.keys[*d as usize].is_some())
                        .count() as u64
                })
            })
            .sum()
    };
    let terms: Vec<(&str, u64)> = query
        .iter()
        .map(|t| (t.as_str(), df(t)))
        .filter(|&(_, d)| d > 0 && (!recall || d < n || n == 1))
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let idf = |d: u64| {
        let d = d as f64;
        (1.0 + (n as f64 - d + 0.5) / (d + 0.5)).ln()
    };
    // A document must match at least one rare term to be recalled.
    let floor = if recall && n >= FLOOR_MIN_CARDS {
        FLOOR * (1.0 + (n as f64 - 1.0 + 0.5) / (1.0 + 0.5)).ln()
    } else {
        0.0
    };
    let mut total = [0u64; FIELDS];
    for (s, &count) in segments.iter().zip(&counts) {
        for (lens, key) in s.index.lens.iter().zip(&s.keys) {
            if key.is_some() {
                for f in 0..FIELDS {
                    total[f] += u64::from(lens[f]);
                }
            }
        }
        if let Some(shared) = &s.shared {
            total[shared.field] += count * u64::from(shared.len);
        }
    }
    let avg = total.map(|t| (t as f64 / n as f64).max(1.0));

    // Key → (score so far, boost). Only documents with a query term appear.
    let mut scores: HashMap<u32, (f64, f64)> = HashMap::new();
    for &(t, d) in &terms {
        let idf = idf(d);
        for s in segments {
            let index = s.index;
            let shared = s.shared_tf(t);
            let mut add = |doc: u32, mut tf: Tf| {
                let Some(key) = s.keys[doc as usize] else {
                    return;
                };
                let mut lens = index.lens[doc as usize];
                if let Some(sh) = &s.shared {
                    tf[sh.field] += shared;
                    lens[sh.field] += sh.len;
                }
                let field_score: f64 = (0..FIELDS)
                    .map(|f| {
                        let tf = f64::from(tf[f]);
                        if tf == 0.0 {
                            return 0.0;
                        }
                        let norm = K1 * (1.0 - B + B * f64::from(lens[f]) / avg[f]);
                        index.weights[f] * tf * (K1 + 1.0) / (tf + norm)
                    })
                    .sum();
                let boost = index.boosts[doc as usize];
                scores.entry(key).or_insert((0.0, boost)).0 += idf * field_score;
            };
            let postings = index.postings.get(t).map_or(&[][..], |p| &p[..]);
            if shared == 0 {
                for &(doc, tf) in postings {
                    add(doc, tf);
                }
            } else {
                // Every document has the term: walk them all.
                let mut postings = postings.iter().peekable();
                for doc in 0..index.len() as u32 {
                    let tf = match postings.next_if(|(d, _)| *d == doc) {
                        Some(&(_, tf)) => tf,
                        None => Tf::default(),
                    };
                    add(doc, tf);
                }
            }
        }
    }

    let mut hits: Vec<(u32, f64)> = scores
        .into_iter()
        .map(|(key, (score, boost))| (key, score * boost))
        .filter(|&(_, score)| score >= floor && score > 0.0)
        .collect();
    hits.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    hits.truncate(limit);
    hits
}

/// Rank `docs` against `query` by BM25F, best first, at most `limit`, as
/// `(index into docs, score)`. Equal scores keep the input order, so callers
/// order `docs` by their own tie key. Weights are the first document's.
pub fn rank_docs(docs: &[Doc], query: &str, limit: usize, recall: bool) -> Vec<(usize, f64)> {
    let Some(first) = docs.first() else {
        return Vec::new();
    };
    let index = Index::new(first.weights, docs);
    let segment = Segment {
        index: &index,
        keys: (0..docs.len() as u32).map(Some).collect(),
        shared: None,
    };
    rank_segments(&[segment], query, limit, recall)
        .into_iter()
        .map(|(key, score)| (key as usize, score))
        .collect()
}

/// Split text into the terms a query matches by: lowercase words of 2+ chars
/// in any alphabet (`naïve` is one word, `Привет` folds to `привет`), and CJK
/// bigrams — Han, kana and Hangul only, a lone char its own term. An
/// `_`-joined word also emits its 2+-char parts, so `session_search` matches
/// `session` and `search`. Fullwidth ASCII and the ideographic space fold to
/// halfwidth first, so `ＡＢＣ２０２４` tokenizes like `abc2024`.
pub fn tokens(text: &str) -> Vec<String> {
    scan(text, false).into_iter().map(|(t, _)| t).collect()
}

/// Tokenize for one side of a ranking. `doc` additionally emits a CJK run's
/// single chars as overlap terms (marked `true`), so a one-char query can hit
/// a document. Like Lucene's `CJKBigramFilter`, they sit at the same position
/// as their bigrams and are excluded from length normalization; everything
/// else counts toward the field's token length.
fn scan(text: &str, doc: bool) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut run: Vec<char> = Vec::new();
    for c in text.chars().map(fold_fullwidth) {
        if is_cjk(c) {
            flush_word(&mut word, &mut out);
            run.push(c);
        } else if c.is_alphanumeric() || c == '_' {
            flush(&mut run, &mut out, doc);
            word.extend(c.to_lowercase());
        } else {
            flush_word(&mut word, &mut out);
            flush(&mut run, &mut out, doc);
        }
    }
    flush_word(&mut word, &mut out);
    flush(&mut run, &mut out, doc);
    out
}

/// The scripts split into bigrams: Han, kana and Hangul. Every other
/// alphabet (Cyrillic, Greek, Latin-1 ...) forms ordinary words.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}' // hiragana, katakana
        | '\u{31F0}'..='\u{31FF}' // katakana phonetic extensions
        | '\u{3400}'..='\u{4DBF}' // CJK extension A
        | '\u{4E00}'..='\u{9FFF}' // unified ideographs
        | '\u{F900}'..='\u{FAFF}' // compatibility ideographs
        | '\u{AC00}'..='\u{D7AF}' // hangul syllables
        | '\u{20000}'..='\u{2FFFF}', // supplementary ideograph planes
    )
}

/// Fold fullwidth ASCII (U+FF01–FF5E) by subtracting 0xFEE0, and the
/// ideographic space to a plain space.
fn fold_fullwidth(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

/// Thresholds count chars, not bytes: a lone `ж` (2 bytes) is no term.
fn flush_word(word: &mut String, out: &mut Vec<(String, bool)>) {
    if word.chars().count() >= 2 {
        if word.contains('_') {
            out.extend(
                word.split('_')
                    .filter(|p| p.chars().count() >= 2)
                    .map(|p| (p.to_owned(), false)),
            );
        }
        out.push((std::mem::take(word), false));
    }
    word.clear();
}

fn flush(run: &mut Vec<char>, out: &mut Vec<(String, bool)>, doc: bool) {
    match run.len() {
        0 => {}
        1 => out.push((run[0].to_string(), false)),
        _ => {
            if doc {
                out.extend(run.iter().map(|&c| (c.to_string(), true)));
            }
            out.extend(run.windows(2).map(|p| (p.iter().collect(), false)));
        }
    }
    run.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_card;

    fn cards(entries: &[(&str, &str)]) -> BTreeMap<String, Card> {
        entries
            .iter()
            .map(|(id, text)| ((*id).to_owned(), parse_card(id, text)))
            .collect()
    }

    struct Hit<'a> {
        card: &'a Card,
        score: f64,
    }

    /// Rank `cards` as one index, each by its own path, ties by id.
    fn ranked<'a>(
        cards: &'a BTreeMap<String, Card>,
        query: &str,
        limit: usize,
        recall: bool,
    ) -> Vec<Hit<'a>> {
        let list: Vec<&Card> = cards.values().collect();
        let docs: Vec<Doc> = list.iter().map(|c| Doc::card(c, &c.path)).collect();
        rank_docs(&docs, query, limit, recall)
            .into_iter()
            .map(|(i, score)| Hit {
                card: list[i],
                score,
            })
            .collect()
    }

    fn rank<'a>(cards: &'a BTreeMap<String, Card>, query: &str, limit: usize) -> Vec<Hit<'a>> {
        ranked(cards, query, limit, true)
    }

    fn search<'a>(cards: &'a BTreeMap<String, Card>, query: &str, limit: usize) -> Vec<Hit<'a>> {
        ranked(cards, query, limit, false)
    }

    fn ids(hits: Vec<Hit<'_>>) -> Vec<String> {
        hits.into_iter().map(|h| h.card.path.clone()).collect()
    }

    #[test]
    fn keywords_outrank_id_description_and_body() {
        // The same word in each field, nothing else to tilt the lengths.
        let cards = cards(&[
            ("kw", "---\nkeywords: [deploy]\n---\nx\n"),
            ("ops/deploy", "---\n---\nx\n"),
            ("desc", "---\ndescription: deploy\n---\nx\n"),
            ("body", "---\ndescription: unrelated\n---\ndeploy\n"),
            ("none", "---\ndescription: other\n---\nx\n"),
        ]);
        assert_eq!(
            ids(rank(&cards, "deploy", 10)),
            ["kw", "ops/deploy", "desc", "body"]
        );
    }

    #[test]
    fn cjk_runs_match_by_bigram() {
        let cards = cards(&[("proxy", "---\ndescription: 代理服务器配置\n---\n正文\n")]);
        assert_eq!(ids(rank(&cards, "代理怎么配", 10)), ["proxy"]);
        // Recall ignores one-char query terms: a reply like `服` or `好` must
        // not recall every card containing the char.
        assert!(rank(&cards, "服", 10).is_empty());
        assert!(rank(&cards, "好", 10).is_empty());
        // Explicit search keeps them, and documents index a run's chars.
        assert_eq!(ids(search(&cards, "服", 10)), ["proxy"]);
    }

    #[test]
    fn cjk_text_is_as_long_as_its_bigrams() {
        // `我是人类` is 3 bigrams long, like three words: the overlap
        // unigrams it also indexes must not lengthen it, so CJK cards are not
        // dwarfed by English ones in length normalization. A lone char is one
        // token, like one word.
        for (cjk, words) in [("我是人类", "one two three"), ("我", "one")] {
            let a = format!("---\n---\ndeploy {cjk}\n");
            let b = format!("---\n---\ndeploy {words}\n");
            let cards = cards(&[("a", &a), ("b", &b), ("c", "---\n---\nother\n")]);
            let hits = search(&cards, "deploy", 10);
            assert_eq!(hits.len(), 2, "{cjk}");
            assert_eq!(hits[0].score, hits[1].score, "{cjk}");
        }
    }

    #[test]
    fn other_alphabets_join_words_and_fullwidth_folds() {
        let cards = cards(&[(
            "words",
            "---\ndescription: naïve Привет\n---\nＡＢＣ ２０２４ 第3章\n",
        )]);
        assert_eq!(ids(search(&cards, "naïve", 10)), ["words"]);
        assert_eq!(ids(search(&cards, "привет", 10)), ["words"]);
        assert_eq!(ids(search(&cards, "ПРИВЕТ", 10)), ["words"]);
        // Fullwidth ASCII folds to halfwidth on both sides of the ranking.
        assert_eq!(ids(search(&cards, "abc", 10)), ["words"]);
        assert_eq!(ids(search(&cards, "ＡＢＣ", 10)), ["words"]);
        assert_eq!(ids(search(&cards, "2024", 10)), ["words"]);
        // Mixed-script text tokenizes without panic.
        assert_eq!(ids(search(&cards, "第3章", 10)), ["words"]);
    }

    #[test]
    fn word_thresholds_count_chars_not_bytes() {
        // `ж` is 2 bytes but 1 char, so it is no term on either side.
        let cards = cards(&[("zh", "---\ndescription: ж\n---\nж\n")]);
        assert!(search(&cards, "ж", 10).is_empty());
    }

    #[test]
    fn underscore_parts_match() {
        let cards = cards(&[
            ("joined", "---\n---\nrun the session_search tool\n"),
            ("split", "---\n---\nsession and search\n"),
            ("other", "---\n---\nunrelated\n"),
        ]);
        // A `_` word also indexes its parts: `session` finds it.
        let hits = ids(rank(&cards, "session", 10));
        assert!(hits.contains(&"joined".to_owned()), "{hits:?}");
        assert!(hits.contains(&"split".to_owned()), "{hits:?}");
        assert!(!hits.contains(&"other".to_owned()), "{hits:?}");
        // A `_` query also indexes its parts, so it reaches both spellings.
        let hits = ids(rank(&cards, "session_search", 10));
        assert!(hits.contains(&"joined".to_owned()), "{hits:?}");
        assert!(hits.contains(&"split".to_owned()), "{hits:?}");
    }

    #[test]
    fn the_relevance_floor_gates_on_rare_terms() {
        // 20 cards: one mentions `rare`, the rest mention `common`.
        let mut entries: Vec<(String, String)> = (0..20)
            .map(|i| {
                let body = if i == 0 { "rare" } else { "common" };
                (format!("c{i}"), format!("---\n---\n{body}\n"))
            })
            .collect();
        entries.sort();
        let docs: BTreeMap<String, Card> = entries
            .iter()
            .map(|(id, text)| (id.clone(), parse_card(id, text)))
            .collect();

        // A term in 19 of 20 docs is far below the floor.
        assert!(rank(&docs, "common", 10).is_empty());
        // The unique term passes ...
        assert_eq!(ids(rank(&docs, "rare", 10)), ["c0"]);
        // ... and carries a card that also matches a common term.
        assert_eq!(ids(rank(&docs, "rare common", 10)), ["c0"]);
        // A smaller corpus has no floor (5 cards, a term in 2 of them).
        let small = cards(&[
            ("a", "---\n---\nalpha\n"),
            ("b", "---\n---\nalpha\n"),
            ("c", "---\n---\nbeta\n"),
            ("d", "---\n---\ngamma\n"),
            ("e", "---\n---\ndelta\n"),
        ]);
        assert_eq!(rank(&small, "alpha", 10).len(), 2);
    }

    #[test]
    fn common_terms_and_limits() {
        let cards = cards(&[
            ("a", "---\nkeywords: [alpha, shared]\n---\nx\n"),
            ("b", "---\nkeywords: [beta, shared]\n---\nx\n"),
            ("c", "---\nkeywords: [gamma, shared]\n---\nx\n"),
        ]);
        // "shared" is in every card and carries no signal.
        assert!(rank(&cards, "shared", 10).is_empty());
        assert_eq!(ids(rank(&cards, "alpha beta", 10)), ["a", "b"]);
        assert_eq!(rank(&cards, "alpha", 1).len(), 1);
        assert!(rank(&cards, "", 10).is_empty());
        assert!(rank(&cards, "!!!", 10).is_empty());
    }
}
