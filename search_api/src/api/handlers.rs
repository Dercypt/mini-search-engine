use std::sync::Arc;
use std::time::Instant;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};

use crate::api::state::{
    AppState, HealthResponse, SearchHit, SearchParams, SearchResponse, SuggestParams,
};
use crate::ranking::{
    DEFAULT_ALPHA, DEFAULT_BETA, execute_wand_search, generate_dynamic_snippet, parse_query,
};

pub async fn ui_handler() -> Html<&'static str> {
    Html(include_str!("../index.html"))
}

pub async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        total_documents: state.index.total_docs,
        vocabulary_size: state.index.num_terms as usize,
    })
}

pub async fn suggest_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SuggestParams>,
) -> impl IntoResponse {
    let prefix = params.q.unwrap_or_default().trim().to_string();
    let limit = params.limit.unwrap_or(5).clamp(1, 20);

    if prefix.is_empty() {
        return Json(Vec::<String>::new());
    }

    let suggestions = state.surface_dictionary.suggest_prefix(&prefix, limit);
    Json(suggestions)
}

pub async fn search_handler(
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
                did_you_mean: None,
                results: Vec::new(),
            }),
        );
    }

    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let start_time = Instant::now();

    let parsed_query = parse_query(&query_str, &state.pipeline);
    let all_terms = parsed_query.all_terms();
    if all_terms.is_empty() {
        return (
            StatusCode::OK,
            Json(SearchResponse {
                query: query_str,
                total_hits: 0,
                page,
                limit,
                total_pages: 0,
                execution_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
                did_you_mean: None,
                results: Vec::new(),
            }),
        );
    }

    let alpha = params.alpha.unwrap_or(DEFAULT_ALPHA).clamp(0.0, 1.0);
    let beta = DEFAULT_BETA;
    let top_k = (page * limit).max(100);

    let wand_result = execute_wand_search(
        &state.index,
        &state.dictionary,
        &parsed_query,
        alpha,
        beta,
        top_k,
    );
    let ranked = wand_result.ranked;
    let total_hits = wand_result.total_hits;
    let total_pages = total_hits.div_ceil(limit);

    // Fuzzy matching fallback if zero hits were scored
    let did_you_mean = if total_hits == 0 {
        let mut suggestions = Vec::new();
        let query_words = state.pipeline.extract_surface_words(&query_str);
        let candidate_sources = if !query_words.is_empty() {
            query_words
        } else {
            all_terms
        };
        for term in &candidate_sources {
            let fuzzy_candidates = state.surface_dictionary.fuzzy_terms(term, 2, 3);
            suggestions.extend(fuzzy_candidates);
        }
        suggestions.dedup();
        if !suggestions.is_empty() {
            Some(suggestions)
        } else {
            None
        }
    } else {
        None
    };

    // Documented: total_hits tracks evaluated matching documents under WAND pruning,
    // while ranked holds top-k scored documents. Pagination is safely bounded
    // against ranked.len() to guarantee no runtime slice out-of-bounds panics.
    let offset = (page - 1) * limit;
    let paged_results = paginate_ranked_results(&ranked, page, limit);

    let mut hits = Vec::with_capacity(paged_results.len());
    let raw_tokens: Vec<String> = query_str
        .replace('"', " ")
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();

    for (rank_idx, (doc_id, score)) in paged_results.into_iter().enumerate() {
        if let Some(meta) = state.index.get_doc_meta(doc_id) {
            // Read content slice from mmapDocStore on demand - zero document content stored in RAM
            let snippet = if let Some(content) = state.doc_store.get_content(doc_id) {
                generate_dynamic_snippet(content, &raw_tokens, 180)
            } else {
                "No preview text available.".to_string()
            };

            hits.push(SearchHit {
                rank: offset + rank_idx + 1,
                doc_id: meta.hex_id.to_string(),
                score: (score * 10000.0).round() / 10000.0,
                pagerank: Some((meta.pagerank * 1_000_000.0).round() / 1_000_000.0),
                title: meta.title.to_string(),
                url: meta.url.to_string(),
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
            did_you_mean,
            results: hits,
        }),
    )
}

#[inline]
fn paginate_ranked_results(ranked: &[(u32, f64)], page: usize, limit: usize) -> Vec<(u32, f64)> {
    let offset = (page.saturating_sub(1)) * limit;
    if offset < ranked.len() {
        ranked[offset..(offset + limit).min(ranked.len())].to_vec()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::storage::{
        BinaryIndexWriter, DocMetadata, DocStoreWriter, MmapDocStore, MmapIndex, Posting,
        TermDictionary,
    };
    use engine_core::tokenizer::TokenizerPipeline;
    use fst::MapBuilder;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::BufWriter;
    use std::sync::Arc;

    fn create_test_state(temp_dir: &std::path::Path, id_suffix: &str) -> Arc<AppState> {
        let index_path = temp_dir.join(format!("test_idx_{id_suffix}.bin"));
        let docs_path = temp_dir.join(format!("test_docs_{id_suffix}.bin"));
        let fst_path = temp_dir.join(format!("test_dict_{id_suffix}.fst"));
        let surface_fst_path = temp_dir.join(format!("test_surface_{id_suffix}.fst"));

        // 1. Doc store
        {
            let mut writer = DocStoreWriter::create(&docs_path).expect("create doc store");
            writer
                .append(
                    "doc1",
                    "https://example.com/1",
                    "Search Algorithms",
                    "Engines with algorithmic efficiency and distributed systems.",
                    &[],
                )
                .expect("append doc");
            writer.finish().expect("finish doc store");
        }

        // 2. Binary index with stemmed terms
        let sorted_stemmed = vec![
            "algorithm".to_string(),
            "distribut".to_string(),
            "engin".to_string(),
            "system".to_string(),
        ];
        let mut inverted_index = HashMap::new();
        inverted_index.insert(
            "algorithm".to_string(),
            vec![Posting {
                doc_id: 0,
                term_frequency: 2,
                positions: vec![1, 5],
            }],
        );
        inverted_index.insert(
            "distribut".to_string(),
            vec![Posting {
                doc_id: 0,
                term_frequency: 1,
                positions: vec![7],
            }],
        );
        inverted_index.insert(
            "engin".to_string(),
            vec![Posting {
                doc_id: 0,
                term_frequency: 1,
                positions: vec![3],
            }],
        );
        inverted_index.insert(
            "system".to_string(),
            vec![Posting {
                doc_id: 0,
                term_frequency: 1,
                positions: vec![8],
            }],
        );

        let doc_metadata = vec![DocMetadata {
            internal_id: 0,
            hex_id: "doc1".to_string(),
            url: "https://example.com/1".to_string(),
            title: "Search Algorithms".to_string(),
            length: 10,
            pagerank: 0.5,
        }];

        BinaryIndexWriter::write_index(
            &index_path,
            1,
            10.0,
            &[10],
            &[0.5],
            &doc_metadata,
            &sorted_stemmed,
            &inverted_index,
        )
        .expect("write index");

        // 3. Stemmed dictionary FST
        {
            let file = File::create(&fst_path).expect("create fst");
            let mut builder = MapBuilder::new(BufWriter::new(file)).expect("builder");
            for (i, term) in sorted_stemmed.iter().enumerate() {
                builder.insert(term.as_bytes(), i as u64).expect("insert");
            }
            builder.finish().expect("finish");
        }

        // 4. Surface dictionary FST with unstemmed English words
        let surface_words = vec![
            ("algorithm".to_string(), 10),
            ("algorithmic".to_string(), 5),
            ("algorithms".to_string(), 8),
            ("distributed".to_string(), 4),
            ("distribution".to_string(), 2),
            ("engine".to_string(), 12),
            ("engines".to_string(), 7),
            ("programming".to_string(), 3),
        ];
        let surface_dict =
            TermDictionary::build_from_surface_words(&surface_fst_path, surface_words)
                .expect("build surface dict");

        let index = Arc::new(MmapIndex::open(&index_path).expect("open index"));
        let doc_store = Arc::new(MmapDocStore::open(&docs_path).expect("open docs"));
        let dictionary = Arc::new(TermDictionary::open(&fst_path).expect("open dict"));

        Arc::new(AppState {
            index,
            doc_store,
            pipeline: TokenizerPipeline::new(),
            dictionary,
            surface_dictionary: Arc::new(surface_dict),
        })
    }

    #[tokio::test]
    async fn test_suggest_handler_returns_unstemmed_surface_words() {
        let temp_dir = std::env::temp_dir();
        let state = create_test_state(&temp_dir, &format!("sugg_{}", std::process::id()));

        // Query prefix "algo" with limit 3 (matching README.md example)
        let response = suggest_handler(
            State(state.clone()),
            Query(SuggestParams {
                q: Some("algo".to_string()),
                limit: Some(3),
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions: Vec<String> = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(
            suggestions,
            vec![
                "algorithm".to_string(),
                "algorithmic".to_string(),
                "algorithms".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn test_did_you_mean_returns_unstemmed_surface_words() {
        let temp_dir = std::env::temp_dir();
        let state = create_test_state(&temp_dir, &format!("dym_{}", std::process::id()));

        // Query "engne" which has 0 hits in index
        let response = search_handler(
            State(state.clone()),
            Query(SearchParams {
                q: Some("engne".to_string()),
                page: Some(1),
                limit: Some(10),
                alpha: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let search_resp: SearchResponse = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(search_resp.total_hits, 0);
        let dym = search_resp
            .did_you_mean
            .expect("expected did_you_mean suggestions");
        assert!(dym.contains(&"engine".to_string()));
        // Ensure no truncated stems like "engin" are returned
        assert!(!dym.contains(&"engin".to_string()));
    }

    #[tokio::test]
    async fn test_suggest_handler_edge_cases_empty_casing_and_limits() {
        let temp_dir = std::env::temp_dir();
        let state = create_test_state(&temp_dir, &format!("sugg_edge_{}", std::process::id()));

        // 1. Empty and whitespace-only prefixes
        let empty_res = suggest_handler(
            State(state.clone()),
            Query(SuggestParams {
                q: Some("   ".to_string()),
                limit: Some(5),
            }),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(empty_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert!(suggestions.is_empty());

        let none_res = suggest_handler(
            State(state.clone()),
            Query(SuggestParams {
                q: None,
                limit: Some(5),
            }),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(none_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert!(suggestions.is_empty());

        // 2. Case-insensitivity ("ALGO")
        let case_res = suggest_handler(
            State(state.clone()),
            Query(SuggestParams {
                q: Some("ALGO".to_string()),
                limit: Some(3),
            }),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(case_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            suggestions,
            vec![
                "algorithm".to_string(),
                "algorithmic".to_string(),
                "algorithms".to_string(),
            ]
        );

        // 3. Clamping limit
        let clamped_res = suggest_handler(
            State(state.clone()),
            Query(SuggestParams {
                q: Some("algo".to_string()),
                limit: Some(1),
            }),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(clamped_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let suggestions: Vec<String> = serde_json::from_slice(&body).unwrap();
        assert_eq!(suggestions, vec!["algorithm".to_string()]);
    }

    #[tokio::test]
    async fn test_search_handler_no_did_you_mean_on_positive_hits() {
        let temp_dir = std::env::temp_dir();
        let state = create_test_state(&temp_dir, &format!("hits_{}", std::process::id()));

        // Query "algorithm" which is present in index (total_hits > 0)
        let response = search_handler(
            State(state.clone()),
            Query(SearchParams {
                q: Some("algorithm".to_string()),
                page: Some(1),
                limit: Some(10),
                alpha: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let search_resp: SearchResponse = serde_json::from_slice(&body_bytes).unwrap();

        assert!(search_resp.total_hits > 0);
        assert!(search_resp.did_you_mean.is_none());
    }

    #[test]
    fn test_paginate_ranked_results_bounds() {
        let ranked: Vec<(u32, f64)> = (0..10).map(|i| (i, 1.0 / (i as f64 + 1.0))).collect();

        // Exact first page
        let p1 = paginate_ranked_results(&ranked, 1, 5);
        assert_eq!(p1.len(), 5);
        assert_eq!(p1[0].0, 0);
        assert_eq!(p1[4].0, 4);

        // Second page
        let p2 = paginate_ranked_results(&ranked, 2, 5);
        assert_eq!(p2.len(), 5);
        assert_eq!(p2[0].0, 5);
        assert_eq!(p2[4].0, 9);

        // Third page out of bounds -> empty, no panic
        let p3 = paginate_ranked_results(&ranked, 3, 5);
        assert!(p3.is_empty());

        // Partial final page: page 2 with limit 7 (ranked has 10 items, offset 7, limit 7)
        let p_partial = paginate_ranked_results(&ranked, 2, 7);
        assert_eq!(p_partial.len(), 3);
        assert_eq!(p_partial[0].0, 7);
        assert_eq!(p_partial[2].0, 9);

        // Empty ranked
        let empty_ranked: Vec<(u32, f64)> = Vec::new();
        let p_empty = paginate_ranked_results(&empty_ranked, 1, 10);
        assert!(p_empty.is_empty());
    }

    #[test]
    fn test_paginate_ranked_results_when_total_hits_exceeds_heap() {
        // Simulates WAND pruning where 50 candidates were evaluated (total_hits = 50),
        // but only top 5 were retained in heap (ranked.len() = 5).
        let ranked: Vec<(u32, f64)> = (0..5).map(|i| (i, 10.0 - i as f64)).collect();

        // Requesting page 2 with limit 5 (offset 5 == ranked.len())
        let p2 = paginate_ranked_results(&ranked, 2, 5);
        assert!(p2.is_empty());

        // Requesting page 1 with limit 10 (offset 0, offset + limit = 10 > ranked.len())
        let p1 = paginate_ranked_results(&ranked, 1, 10);
        assert_eq!(p1.len(), 5);
    }
}
