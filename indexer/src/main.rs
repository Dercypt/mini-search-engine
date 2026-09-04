use fst::SetBuilder;
use regex::Regex;
use roaring::RoaringBitmap;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::time::Instant;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawDocument {
    pub id: String,
    pub url: String,
    pub title: String,
    pub content: String,
    pub links: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Posting {
    pub doc_id: u32,
    pub term_frequency: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DocMetadata {
    pub internal_id: u32,
    pub hex_id: String,
    pub url: String,
    pub title: String,
    pub length: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexStore {
    pub total_docs: u32,
    pub avg_doc_length: f64,
    pub doc_meta: HashMap<u32, DocMetadata>,
    pub inverted_index: HashMap<String, Vec<Posting>>,
    #[serde(skip)]
    pub term_bitmaps: HashMap<String, RoaringBitmap>,
}

pub struct TokenizerPipeline {
    stemmer: Stemmer,
    stop_words: HashSet<&'static str>,
    regex: Regex,
}

impl TokenizerPipeline {
    pub fn new() -> Self {
        let stop_words: HashSet<&'static str> = [
            "a",
            "about",
            "above",
            "after",
            "again",
            "against",
            "all",
            "am",
            "an",
            "and",
            "any",
            "are",
            "aren't",
            "as",
            "at",
            "be",
            "because",
            "been",
            "before",
            "being",
            "below",
            "between",
            "both",
            "but",
            "by",
            "can't",
            "cannot",
            "could",
            "couldn't",
            "did",
            "didn't",
            "do",
            "does",
            "doesn't",
            "doing",
            "don't",
            "down",
            "during",
            "each",
            "few",
            "for",
            "from",
            "further",
            "had",
            "hadn't",
            "has",
            "hasn't",
            "have",
            "haven't",
            "having",
            "he",
            "he'd",
            "he'll",
            "he's",
            "her",
            "here",
            "here's",
            "hers",
            "herself",
            "him",
            "himself",
            "his",
            "how",
            "how's",
            "i",
            "i'd",
            "i'll",
            "i'm",
            "i've",
            "if",
            "in",
            "into",
            "is",
            "isn't",
            "it",
            "it's",
            "its",
            "itself",
            "let's",
            "me",
            "more",
            "most",
            "mustn't",
            "my",
            "myself",
            "no",
            "nor",
            "not",
            "of",
            "off",
            "on",
            "once",
            "only",
            "or",
            "other",
            "ought",
            "our",
            "ours",
            "ourselves",
            "out",
            "over",
            "own",
            "same",
            "shan't",
            "she",
            "she'd",
            "she'll",
            "she's",
            "should",
            "shouldn't",
            "so",
            "some",
            "such",
            "than",
            "that",
            "that's",
            "the",
            "their",
            "theirs",
            "them",
            "themselves",
            "then",
            "there",
            "there's",
            "these",
            "they",
            "they'd",
            "they'll",
            "they're",
            "they've",
            "this",
            "those",
            "through",
            "to",
            "too",
            "under",
            "until",
            "up",
            "very",
            "was",
            "wasn't",
            "we",
            "we'd",
            "we'll",
            "we're",
            "we've",
            "were",
            "weren't",
            "what",
            "what's",
            "when",
            "when's",
            "where",
            "where's",
            "which",
            "while",
            "who",
            "who's",
            "whom",
            "why",
            "why's",
            "with",
            "won't",
            "would",
            "wouldn't",
            "you",
            "you'd",
            "you'll",
            "you're",
            "you've",
            "your",
            "yours",
            "yourself",
            "yourselves",
        ]
        .into_iter()
        .collect();

        Self {
            stemmer: Stemmer::create(Algorithm::English),
            stop_words,
            regex: Regex::new(r"[^a-zA-Z0-9\s]+").unwrap(),
        }
    }

    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let cleaned = self.regex.replace_all(text, " ").to_lowercase();
        cleaned
            .split_whitespace()
            .filter(|token| token.len() > 1 && !self.stop_words.contains(token))
            .map(|token| self.stemmer.stem(token).to_string())
            .collect()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw_docs_path = "../crawler/documents.json";
    let output_index_path = "index.json";
    let output_fst_path = "dictionary.fst";

    println!("Loading documents from {}...", raw_docs_path);
    let start_time = Instant::now();

    let file = File::open(raw_docs_path)?;
    let reader = BufReader::new(file);
    let docs: Vec<RawDocument> = serde_json::from_reader(reader)?;
    println!(
        "Loaded {} documents in {:?}",
        docs.len(),
        start_time.elapsed()
    );

    let pipeline = TokenizerPipeline::new();
    let mut index_store = build_index(&docs, &pipeline);

    println!("\n--- Indexing Statistics ---");
    println!("Total Documents: {}", index_store.total_docs);
    println!(
        "Unique Vocabulary Terms: {}",
        index_store.inverted_index.len()
    );
    println!(
        "Average Document Length: {:.2} terms",
        index_store.avg_doc_length
    );

    // Save index
    let save_start = Instant::now();
    let json_bytes = serde_json::to_vec_pretty(&index_store)?;
    let mut out_file = File::create(output_index_path)?;
    out_file.write_all(&json_bytes)?;
    println!("Saved {} in {:?}", output_index_path, save_start.elapsed());

    // Build and Save Lexicon FST (Requires lexicographical ordering)
    let fst_start = Instant::now();
    let mut sorted_terms: Vec<String> = index_store.inverted_index.keys().cloned().collect();
    sorted_terms.sort();
    sorted_terms.dedup();

    let fst_file = File::create(output_fst_path)?;
    let fst_writer = BufWriter::new(fst_file);
    let mut builder = SetBuilder::new(fst_writer)?;

    for term in &sorted_terms {
        builder.insert(term.as_bytes())?;
    }
    builder.finish()?;
    println!(
        "Compiled and saved {} terms to {} in {:?}",
        sorted_terms.len(),
        output_fst_path,
        fst_start.elapsed()
    );

    // Test BM25 Query with Roaring Bitmaps
    let test_queries = ["search engine", "page rank algorithm", "open source"];
    for query in test_queries {
        println!("\n--- Test Query: \"{}\" ---", query);
        let results = search_bm25(query, &mut index_store, &pipeline, 5);
        for (rank, (doc, score)) in results.iter().enumerate() {
            println!("{}. [{:.4}] {} ({})", rank + 1, score, doc.title, doc.url);
        }
    }

    Ok(())
}

fn build_index(docs: &[RawDocument], pipeline: &TokenizerPipeline) -> IndexStore {
    let mut inverted_index: HashMap<String, Vec<Posting>> = HashMap::new();
    let mut term_bitmaps: HashMap<String, RoaringBitmap> = HashMap::new();
    let mut doc_meta: HashMap<u32, DocMetadata> = HashMap::new();
    let mut total_terms: usize = 0;

    for (idx, doc) in docs.iter().enumerate() {
        let internal_id = idx as u32;

        // Title terms receive double weight
        let mut title_terms = pipeline.tokenize(&doc.title_or_default(&doc.title));
        let content_terms = pipeline.tokenize(&doc.content);

        let mut all_terms = Vec::with_capacity(title_terms.len() * 2 + content_terms.len());
        all_terms.append(&mut title_terms.clone());
        all_terms.append(&mut title_terms);
        all_terms.extend(content_terms);

        let doc_length = all_terms.len() as u32;
        total_terms += doc_length as usize;

        doc_meta.insert(
            internal_id,
            DocMetadata {
                internal_id,
                hex_id: doc.id.clone(),
                url: doc.url.clone(),
                title: doc.title.clone(),
                length: doc_length,
            },
        );

        // Calculate term frequency for this document
        let mut tf_map: HashMap<String, u32> = HashMap::new();
        for term in all_terms {
            *tf_map.entry(term).or_insert(0) += 1;
        }

        for (term, tf) in tf_map {
            inverted_index
                .entry(term.clone())
                .or_default()
                .push(Posting {
                    doc_id: internal_id,
                    term_frequency: tf,
                });

            term_bitmaps.entry(term).or_default().insert(internal_id);
        }
    }

    let avg_doc_length = if !docs.is_empty() {
        total_terms as f64 / docs.len() as f64
    } else {
        0.0
    };

    IndexStore {
        total_docs: docs.len() as u32,
        avg_doc_length,
        doc_meta,
        inverted_index,
        term_bitmaps,
    }
}

trait TitleHelper {
    fn title_or_default<'a>(&'a self, fallback: &'a str) -> &'a str;
}

impl TitleHelper for RawDocument {
    fn title_or_default<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.title.is_empty() {
            fallback
        } else {
            &self.title
        }
    }
}

pub fn search_bm25<'a>(
    query: &str,
    index: &'a mut IndexStore,
    pipeline: &TokenizerPipeline,
    top_k: usize,
) -> Vec<(&'a DocMetadata, f64)> {
    let k1 = 1.5;
    let b = 0.75;
    let query_terms = pipeline.tokenize(query);
    if query_terms.is_empty() {
        return Vec::new();
    }

    let mut scores: HashMap<u32, f64> = HashMap::new();
    let n = index.total_docs as f64;

    for term in &query_terms {
        if let Some(postings) = index.inverted_index.get(term) {
            let n_q = postings.len() as f64;
            let idf = ((n - n_q + 0.5) / (n_q + 0.5) + 1.0).ln();

            for posting in postings {
                let meta = &index.doc_meta[&posting.doc_id];
                let tf = posting.term_frequency as f64;
                let doc_len = meta.length as f64;

                let num = tf * (k1 + 1.0);
                let denom = tf + k1 * (1.0 - b + b * (doc_len / index.avg_doc_length));
                let term_score = idf * (num / denom);

                *scores.entry(posting.doc_id).or_insert(0.0) += term_score;
            }
        }
    }

    let mut ranked: Vec<(&DocMetadata, f64)> = scores
        .into_iter()
        .map(|(doc_id, score)| (&index.doc_meta[&doc_id], score))
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(top_k);
    ranked
}
