use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// --- Data Models ---

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
}

// --- Tokenizer Pipeline ---

#[derive(Clone)]
pub struct TokenizerPipeline {
    stemmer: Arc<Stemmer>,
    stop_words: Arc<HashSet<&'static str>>,
    regex: Regex,
}

impl TokenizerPipeline {
    pub fn new() -> Self {
        let stop_words: HashSet<&'static str> = [
            "a", "about", "above", "after", "again", "against", "all", "am", "an", "and",
            "any", "are", "aren't", "as", "at", "be", "because", "been", "before", "being",
            "below", "between", "both", "but", "by", "can't", "cannot", "could", "couldn't",
            "did", "didn't", "do", "does", "doesn't", "doing", "don't", "down", "during",
            "each", "few", "for", "from", "further", "had", "hadn't", "has", "hasn't", "have",
            "haven't", "having", "he", "he'd", "he'll", "he's", "her", "here", "here's",
            "hers", "herself", "him", "himself", "his", "how", "how's", "i", "i'd", "i'll",
            "i'm", "i've", "if", "in", "into", "is", "isn't", "it", "it's", "its", "itself",
            "let's", "me", "more", "most", "mustn't", "my", "myself", "no", "nor", "not", "of",
            "off", "on", "once", "only", "or", "other", "ought", "our", "ours", "ourselves",
            "out", "over", "own", "same", "shan't", "she", "she'd", "she'll", "she's",
            "should", "shouldn't", "so", "some", "such", "than", "that", "that's", "the",
            "their", "theirs", "them", "themselves", "then", "there", "there's", "these",
            "they", "they'd", "they'll", "they're", "they've", "this", "those", "through",
            "to", "too", "under", "until", "up", "very", "was", "wasn't", "we", "we'd", "we'll",
            "we're", "we've", "were", "weren't", "what", "what's", "when", "when's", "where",
            "where's", "which", "while", "who", "who's", "whom", "why", "why's", "with",
            "won't", "would", "wouldn't", "you", "you'd", "you'll", "you're", "you've", "your",
            "yours", "yourself", "yourselves",
        ]
        .into_iter()
        .collect();

        Self {
            stemmer: Arc::new(Stemmer::create(Algorithm::English)),
            stop_words: Arc::new(stop_words),
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

// --- App State ---

pub struct AppState {
    pub index: IndexStore,
    pub raw_docs_by_id: HashMap<String, RawDocument>,
    pub pipeline: TokenizerPipeline,
}

// --- Request / Response DTOs ---

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub page: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub rank: usize,
    pub doc_id: String,
    pub score: f64,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub total_hits: usize,
    pub page: usize,
    pub limit: usize,
    pub total_pages: usize,
    pub execution_time_ms: f64,
    pub results: Vec<SearchHit>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub total_documents: u32,
    pub vocabulary_size: usize,
}

// --- Main Server ---

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let index_path = "../indexer/index.json";
    let docs_path = "../crawler/documents.json";

    println!("Loading Inverted Index from {}...", index_path);
    let index_file = File::open(index_path)?;
    let index: IndexStore = serde_json::from_reader(BufReader::new(index_file))?;

    println!("Loading Raw Documents from {}...", docs_path);
    let docs_file = File::open(docs_path)?;
    let docs_vec: Vec<RawDocument> = serde_json::from_reader(BufReader::new(docs_file))?;

    let mut raw_docs_by_id = HashMap::with_capacity(docs_vec.len());
    for doc in docs_vec {
        raw_docs_by_id.insert(doc.id.clone(), doc);
    }

    let pipeline = TokenizerPipeline::new();
    let state = Arc::new(AppState {
        index,
        raw_docs_by_id,
        pipeline,
    });

    println!(
        "Engine ready! Indexed {} documents across {} terms.",
        state.index.total_docs,
        state.index.inverted_index.len()
    );

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/api/search", get(search_handler))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("Search API listening on http://localhost:8080");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// --- Handlers ---

async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        total_documents: state.index.total_docs,
        vocabulary_size: state.index.inverted_index.len(),
    })
}

async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let query_str = params.q.unwrap_or_default().trim().to_string();
    if query_str.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(SearchResponse {
                query: "".to_string(),
                total_hits: 0,
                page: 1,
                limit: 10,
                total_pages: 0,
                execution_time_ms: 0.0,
                results: Vec::new(),
            }),
        );
    }

    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let start_time = Instant::now();

    // 1. Tokenize query
    let query_terms = state.pipeline.tokenize(&query_str);
    if query_terms.is_empty() {
        return (
            StatusCode::OK,
            Json(SearchResponse {
                query: query_str,
                total_hits: 0,
                page,
                limit,
                total_pages: 0,
                execution_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
                results: Vec::new(),
            }),
        );
    }

    // 2. BM25 Relevance Scoring
    let k1 = 1.5;
    let b = 0.75;
    let n = state.index.total_docs as f64;
    let mut scores: HashMap<u32, f64> = HashMap::new();

    for term in &query_terms {
        if let Some(postings) = state.index.inverted_index.get(term) {
            let n_q = postings.len() as f64;
            let idf = ((n - n_q + 0.5) / (n_q + 0.5) + 1.0).ln();

            for posting in postings {
                if let Some(meta) = state.index.doc_meta.get(&posting.doc_id) {
                    let tf = posting.term_frequency as f64;
                    let doc_len = meta.length as f64;
                    let num = tf * (k1 + 1.0);
                    let denom = tf + k1 * (1.0 - b + b * (doc_len / state.index.avg_doc_length));
                    let term_score = idf * (num / denom);

                    *scores.entry(posting.doc_id).or_insert(0.0) += term_score;
                }
            }
        }
    }

    let mut ranked: Vec<(u32, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let total_hits = ranked.len();
    let total_pages = (total_hits + limit - 1) / limit;

    // 3. Pagination Window
    let offset = (page - 1) * limit;
    let paged_results = if offset < total_hits {
        ranked[offset..(offset + limit).min(total_hits)].to_vec()
    } else {
        Vec::new()
    };

    // 4. Generate dynamic snippets with contextual window
    let mut hits = Vec::with_capacity(paged_results.len());
    let raw_query_words: Vec<&str> = query_str.split_whitespace().collect();

    for (rank_idx, (doc_id, score)) in paged_results.into_iter().enumerate() {
        if let Some(meta) = state.index.doc_meta.get(&doc_id) {
            let snippet = if let Some(raw_doc) = state.raw_docs_by_id.get(&meta.hex_id) {
                generate_snippet(&raw_doc.content, &raw_query_words, 200)
            } else {
                "No preview text available.".to_string()
            };

            hits.push(SearchHit {
                rank: offset + rank_idx + 1,
                doc_id: meta.hex_id.clone(),
                score: (score * 10000.0).round() / 10000.0,
                title: meta.title.clone(),
                url: meta.url.clone(),
                snippet,
            });
        }
    }

    let elapsed = start_time.elapsed().as_secs_f64() * 1000.0;

    (
        StatusCode::OK,
        Json(SearchResponse {
            query: query_str,
            total_hits,
            page,
            limit,
            total_pages,
            execution_time_ms: (elapsed * 100.0).round() / 100.0,
            results: hits,
        }),
    )
}

// --- Snippet Extractor ---

fn generate_snippet(content: &str, query_words: &[&str], max_len: usize) -> String {
    if content.is_empty() {
        return String::new();
    }

    let lower_content = content.to_lowercase();
    let mut best_pos = None;

    for word in query_words {
        let clean_w = word.to_lowercase();
        if clean_w.len() > 2 {
            if let Some(pos) = lower_content.find(&clean_w) {
                best_pos = Some(pos);
                break;
            }
        }
    }

    let start_idx = match best_pos {
        Some(pos) => pos.saturating_sub(60),
        None => 0,
    };

    // Find clean word boundary
    let safe_start = if start_idx > 0 {
        content[start_idx..]
            .find(' ')
            .map(|offset| start_idx + offset + 1)
            .unwrap_or(start_idx)
    } else {
        0
    };

    let remaining = &content[safe_start..];
    if remaining.len() <= max_len {
        format!("{}...", remaining.trim())
    } else {
        let end_slice = &remaining[..max_len];
        let safe_end = end_slice.rfind(' ').unwrap_or(max_len);
        format!("{}...", &remaining[..safe_end].trim())
    }
}
