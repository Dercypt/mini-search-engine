use std::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::storage::{MmapDocStore, MmapIndex, TermDictionary};
use crate::tokenizer::TokenizerPipeline;

pub struct AppState {
    pub index: Arc<MmapIndex>,
    pub doc_store: Arc<MmapDocStore>,
    pub pipeline: TokenizerPipeline,
    pub dictionary: Arc<TermDictionary>,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub page: Option<usize>,
    pub limit: Option<usize>,
    pub alpha: Option<f64>,
}

#[derive(Deserialize)]
pub struct SuggestParams {
    pub q: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub rank: usize,
    pub doc_id: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagerank: Option<f64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did_you_mean: Option<Vec<String>>,
    pub results: Vec<SearchHit>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub total_documents: u32,
    pub vocabulary_size: usize,
}
