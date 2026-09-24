use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Json,
};

use crate::api::state::{
    AppState, HealthResponse, SearchHit, SearchParams, SearchResponse, SuggestParams,
};
use crate::ranking::{
    bm25_idf, bm25_tf_weight, final_score, generate_dynamic_snippet, parse_query, phrase_matches,
    ScoredDoc, WandPostingCursor, DEFAULT_ALPHA, DEFAULT_EPSILON,
};
use crate::storage::PostingsIterator;

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

    let suggestions = state.dictionary.suggest_prefix(&prefix, limit);
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

    // Filter documents by phrase requirements if any phrases exist
    let mut phrase_matching_docs: Option<HashSet<u32>> = None;

    for phrase in &parsed_query.phrases {
        if phrase.is_empty() {
            continue;
        }
        let mut term_postings = Vec::with_capacity(phrase.len());
        let mut all_found = true;
        for term in phrase {
            if let Some(term_idx) = state.dictionary.get(term)
                && let Some(entry) = state.index.get_term_entry(term_idx as usize)
            {
                let slice = state.index.get_postings_slice(&entry);
                let iter = PostingsIterator::new(slice, entry.doc_freq as usize);
                let map: HashMap<u32, Vec<u32>> = iter.map(|p| (p.doc_id, p.positions)).collect();
                term_postings.push(map);
            } else {
                all_found = false;
                break;
            }
        }

        if !all_found {
            phrase_matching_docs = Some(HashSet::new());
            break;
        }

        let mut matching_in_phrase = HashSet::new();
        if let Some(first_map) = term_postings.first() {
            for (&doc_id, first_positions) in first_map {
                let mut doc_positions: Vec<&[u32]> = Vec::with_capacity(term_postings.len());
                doc_positions.push(first_positions.as_slice());
                let mut in_all = true;
                for next_map in &term_postings[1..] {
                    if let Some(pos) = next_map.get(&doc_id) {
                        doc_positions.push(pos.as_slice());
                    } else {
                        in_all = false;
                        break;
                    }
                }
                if in_all && phrase_matches(&doc_positions) {
                    matching_in_phrase.insert(doc_id);
                }
            }
        }

        phrase_matching_docs = match phrase_matching_docs {
            None => Some(matching_in_phrase),
            Some(existing) => Some(
                existing
                    .intersection(&matching_in_phrase)
                    .copied()
                    .collect(),
            ),
        };

        if let Some(ref docs) = phrase_matching_docs
            && docs.is_empty()
        {
            break;
        }
    }

    let k1 = 1.5;
    let b = 0.75;
    let n = state.index.total_docs as f64;
    let alpha = params.alpha.unwrap_or(DEFAULT_ALPHA).clamp(0.0, 1.0);
    let epsilon = DEFAULT_EPSILON;

    let pr_max = if alpha < 1.0 {
        (1.0 - alpha) * (1.0 + epsilon).ln()
    } else {
        0.0
    };

    let top_k = (page * limit).max(100);

    // Initialize WAND cursors for all query terms
    let mut cursors: Vec<WandPostingCursor> = Vec::with_capacity(all_terms.len());
    for term in &all_terms {
        if let Some(term_idx) = state.dictionary.get(term)
            && let Some(entry) = state.index.get_term_entry(term_idx as usize)
        {
            if entry.doc_freq == 0 {
                continue;
            }
            let n_q = entry.doc_freq as f64;
            let idf = bm25_idf(n, n_q);
            // Universal upper bound on term score contribution
            let max_score = alpha * idf * (k1 + 1.0);
            let slice = state.index.get_postings_slice(&entry);
            let cursor = WandPostingCursor::new(slice, entry.doc_freq as usize, idf, max_score);
            if cursor.has_more {
                cursors.push(cursor);
            }
        }
    }

    let mut heap: BinaryHeap<ScoredDoc> = BinaryHeap::with_capacity(top_k + 1);
    let mut threshold = f64::NEG_INFINITY;

    while !cursors.is_empty() {
        // Sort cursors by current_doc_id
        cursors.sort_unstable_by_key(|c| c.current_doc_id);

        // Find pivot term where accumulated upper bound > threshold - pr_max
        let score_limit = threshold - pr_max;
        let mut accum = 0.0;
        let mut pivot_idx = None;

        for (i, c) in cursors.iter().enumerate() {
            accum += c.max_score;
            if accum > score_limit {
                pivot_idx = Some(i);
                break;
            }
        }

        let Some(p) = pivot_idx else {
            // No remaining document can beat the current threshold
            break;
        };

        let pivot_doc = cursors[p].current_doc_id;

        if cursors[0].current_doc_id == pivot_doc {
            let target_doc = pivot_doc;
            let is_allowed = phrase_matching_docs
                .as_ref()
                .is_none_or(|docs| docs.contains(&target_doc));

            if is_allowed {
                let doc_len = state.index.get_doc_length(target_doc) as f64;
                let mut bm25_total = 0.0;

                for c in cursors.iter_mut() {
                    if c.current_doc_id == target_doc {
                        let tf = c.current_tf as f64;
                        let term_score =
                            c.idf * bm25_tf_weight(tf, doc_len, state.index.avg_doc_length, k1, b);
                        bm25_total += term_score;
                        c.read_next();
                    }
                }

                let pr = state.index.get_pagerank(target_doc);
                let score = final_score(bm25_total, pr, alpha, epsilon);

                if heap.len() < top_k {
                    heap.push(ScoredDoc {
                        doc_id: target_doc,
                        score,
                    });
                    if heap.len() == top_k {
                        threshold = heap.peek().unwrap().score;
                    }
                } else if score > threshold {
                    heap.pop();
                    heap.push(ScoredDoc {
                        doc_id: target_doc,
                        score,
                    });
                    threshold = heap.peek().unwrap().score;
                }
            } else {
                for c in cursors.iter_mut() {
                    if c.current_doc_id == target_doc {
                        c.read_next();
                    }
                }
            }
        } else {
            // Skip uncompetitive documents: advance cursor 0 to at least pivot_doc
            cursors[0].advance_to(pivot_doc);
        }

        cursors.retain(|c| c.has_more);
    }

    let mut ranked: Vec<(u32, f64)> = heap
        .into_iter()
        .map(|hit| (hit.doc_id, hit.score))
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let total_hits = ranked.len();
    let total_pages = total_hits.div_ceil(limit);

    // Fuzzy matching fallback if zero hits were scored
    let did_you_mean = if total_hits == 0 {
        let mut suggestions = Vec::new();
        for term in &all_terms {
            let fuzzy_candidates = state.dictionary.fuzzy_terms(term, 2, 3);
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

    let offset = (page - 1) * limit;
    let paged_results = if offset < total_hits {
        ranked[offset..(offset + limit).min(total_hits)].to_vec()
    } else {
        Vec::new()
    };

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
