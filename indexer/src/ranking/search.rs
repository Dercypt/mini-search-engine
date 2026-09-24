use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::ranking::bm25::{
    bm25_idf, bm25_tf_weight, final_score, parse_query, phrase_matches, DEFAULT_EPSILON,
};
use crate::ranking::wand::{ScoredDoc, WandPostingCursor};
use crate::storage::mmap::{DocMetaRef, MmapIndex, TermDictionary};
use crate::storage::vbyte::PostingsIterator;
use crate::tokenizer::TokenizerPipeline;

pub fn search_combined_mmap<'a>(
    query: &str,
    index: &'a MmapIndex,
    dictionary: &TermDictionary,
    pipeline: &TokenizerPipeline,
    alpha: f64,
    epsilon: f64,
    top_k: usize,
) -> Vec<(DocMetaRef<'a>, f64)> {
    if top_k == 0 {
        return Vec::new();
    }
    let k1 = 1.5;
    let b = 0.75;
    let parsed_query = parse_query(query, pipeline);
    let all_terms = parsed_query.all_terms();
    if all_terms.is_empty() {
        return Vec::new();
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
            if let Some(term_idx) = dictionary.get(term)
                && let Some(entry) = index.get_term_entry(term_idx as usize)
            {
                let slice = index.get_postings_slice(&entry);
                let iter = PostingsIterator::new(slice, entry.doc_freq as usize);
                let map: HashMap<u32, Vec<u32>> = iter.map(|p| (p.doc_id, p.positions)).collect();
                term_postings.push(map);
            } else {
                all_found = false;
                break;
            }
        }

        if !all_found {
            return Vec::new();
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
            return Vec::new();
        }
    }

    let n = index.total_docs as f64;
    let pr_max = if alpha < 1.0 {
        (1.0 - alpha) * (1.0 + epsilon).ln()
    } else {
        0.0
    };

    // Initialize WAND cursors for all query terms
    let mut cursors: Vec<WandPostingCursor<'a>> = Vec::with_capacity(all_terms.len());
    for term in &all_terms {
        if let Some(term_idx) = dictionary.get(term)
            && let Some(entry) = index.get_term_entry(term_idx as usize)
        {
            if entry.doc_freq == 0 {
                continue;
            }
            let n_q = entry.doc_freq as f64;
            let idf = bm25_idf(n, n_q);
            // Universal upper bound on term score contribution
            let max_score = alpha * idf * (k1 + 1.0);
            let slice = index.get_postings_slice(&entry);
            let cursor = WandPostingCursor::new(slice, entry.doc_freq as usize, idf, max_score);
            if cursor.has_more {
                cursors.push(cursor);
            }
        }
    }

    if cursors.is_empty() {
        return Vec::new();
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
                let doc_len = index.get_doc_length(target_doc) as f64;
                let mut bm25_total = 0.0;

                for c in cursors.iter_mut() {
                    if c.current_doc_id == target_doc {
                        let tf = c.current_tf as f64;
                        let term_score =
                            c.idf * bm25_tf_weight(tf, doc_len, index.avg_doc_length, k1, b);
                        bm25_total += term_score;
                        c.read_next();
                    }
                }

                let pr = index.get_pagerank(target_doc);
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

    let mut ranked: Vec<ScoredDoc> = heap.into_vec();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });

    ranked
        .into_iter()
        .filter_map(|hit| index.get_doc_meta(hit.doc_id).map(|meta| (meta, hit.score)))
        .collect()
}

pub fn search_bm25_mmap<'a>(
    query: &str,
    index: &'a MmapIndex,
    dictionary: &TermDictionary,
    pipeline: &TokenizerPipeline,
    top_k: usize,
) -> Vec<(DocMetaRef<'a>, f64)> {
    search_combined_mmap(
        query,
        index,
        dictionary,
        pipeline,
        1.0,
        DEFAULT_EPSILON,
        top_k,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::vbyte::Posting;
    use crate::storage::writer::{BinaryIndexWriter, DocMetadata};
    use fst::MapBuilder;
    use std::fs::File;
    use std::io::BufWriter;

    #[test]
    fn test_quoted_query_search_integration() {
        let test_index_path = "target/test_quoted_search_index.bin";
        let test_fst_path = "target/test_quoted_search_dict.fst";

        let total_docs = 3;
        let avg_doc_length = 10.0;
        let doc_lengths = vec![10, 10, 10];
        let pagerank = vec![0.33, 0.33, 0.34];
        let doc_metadata = vec![
            DocMetadata {
                internal_id: 0,
                hex_id: "doc0".to_string(),
                url: "https://example.com/0".to_string(),
                title: "Distributed Systems Intro".to_string(),
                length: 10,
                pagerank: 0.33,
            },
            DocMetadata {
                internal_id: 1,
                hex_id: "doc1".to_string(),
                url: "https://example.com/1".to_string(),
                title: "Systems That Are Distributed".to_string(),
                length: 10,
                pagerank: 0.33,
            },
            DocMetadata {
                internal_id: 2,
                hex_id: "doc2".to_string(),
                url: "https://example.com/2".to_string(),
                title: "Unrelated Topic".to_string(),
                length: 10,
                pagerank: 0.34,
            },
        ];

        let mut inverted_index = HashMap::new();
        inverted_index.insert(
            "distribut".to_string(),
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 1,
                    positions: vec![0],
                },
                Posting {
                    doc_id: 1,
                    term_frequency: 1,
                    positions: vec![4],
                },
            ],
        );
        inverted_index.insert(
            "system".to_string(),
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 1,
                    positions: vec![1],
                },
                Posting {
                    doc_id: 1,
                    term_frequency: 1,
                    positions: vec![0],
                },
            ],
        );

        let mut sorted_terms = vec!["distribut".to_string(), "system".to_string()];
        sorted_terms.sort();

        BinaryIndexWriter::write_index(
            test_index_path,
            total_docs,
            avg_doc_length,
            &doc_lengths,
            &pagerank,
            &doc_metadata,
            &sorted_terms,
            &inverted_index,
        )
        .expect("write_index failed");

        let fst_file = File::create(test_fst_path).expect("create fst failed");
        let fst_writer = BufWriter::new(fst_file);
        let mut builder = MapBuilder::new(fst_writer).expect("mapbuilder failed");
        for (term_idx, term) in sorted_terms.iter().enumerate() {
            builder
                .insert(term.as_bytes(), term_idx as u64)
                .expect("insert term");
        }
        builder.finish().expect("finish fst");

        let mmap_index = MmapIndex::open(test_index_path).expect("open index failed");
        let dictionary = TermDictionary::open(test_fst_path).expect("open dict failed");
        let pipeline = TokenizerPipeline::new();

        // 1. Quoted query: "distributed systems" -> only doc 0 matches
        let quoted_res = search_combined_mmap(
            "\"distributed systems\"",
            &mmap_index,
            &dictionary,
            &pipeline,
            1.0,
            DEFAULT_EPSILON,
            10,
        );
        assert_eq!(quoted_res.len(), 1);
        assert_eq!(quoted_res[0].0.hex_id, "doc0");

        // 2. Reversed quoted query: "systems distributed" -> 0 hits
        let rev_res = search_combined_mmap(
            "\"systems distributed\"",
            &mmap_index,
            &dictionary,
            &pipeline,
            1.0,
            DEFAULT_EPSILON,
            10,
        );
        assert_eq!(rev_res.len(), 0);

        // 3. Unquoted query: distributed systems -> both doc 0 and doc 1 match
        let unquoted_res = search_combined_mmap(
            "distributed systems",
            &mmap_index,
            &dictionary,
            &pipeline,
            1.0,
            DEFAULT_EPSILON,
            10,
        );
        assert_eq!(unquoted_res.len(), 2);

        let _ = std::fs::remove_file(test_index_path);
        let _ = std::fs::remove_file(test_fst_path);
    }

    #[test]
    fn test_wand_multi_term_top_k_pruning() {
        let test_index_path = "target/test_wand_prune_index.bin";
        let test_fst_path = "target/test_wand_prune_dict.fst";

        let total_docs = 6;
        let avg_doc_length = 20.0;
        let doc_lengths = vec![20; 6];
        let pagerank = vec![1.0 / 6.0; 6];
        let doc_metadata: Vec<DocMetadata> = (0..6)
            .map(|i| DocMetadata {
                internal_id: i,
                hex_id: format!("doc{}", i),
                url: format!("https://example.com/{}", i),
                title: format!("Doc Title {}", i),
                length: 20,
                pagerank: 1.0 / 6.0,
            })
            .collect();

        let mut inverted_index = HashMap::new();
        inverted_index.insert(
            "alpha".to_string(),
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 5,
                    positions: vec![0, 1, 2, 3, 4],
                },
                Posting {
                    doc_id: 1,
                    term_frequency: 5,
                    positions: vec![0, 1, 2, 3, 4],
                },
                Posting {
                    doc_id: 2,
                    term_frequency: 1,
                    positions: vec![0],
                },
            ],
        );
        inverted_index.insert(
            "beta".to_string(),
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 5,
                    positions: vec![0, 1, 2, 3, 4],
                },
                Posting {
                    doc_id: 1,
                    term_frequency: 4,
                    positions: vec![0, 1, 2, 3],
                },
                Posting {
                    doc_id: 3,
                    term_frequency: 1,
                    positions: vec![0],
                },
            ],
        );
        inverted_index.insert(
            "gamma".to_string(),
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 5,
                    positions: vec![0, 1, 2, 3, 4],
                },
                Posting {
                    doc_id: 4,
                    term_frequency: 1,
                    positions: vec![0],
                },
                Posting {
                    doc_id: 5,
                    term_frequency: 1,
                    positions: vec![0],
                },
            ],
        );

        let mut sorted_terms = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        sorted_terms.sort();

        BinaryIndexWriter::write_index(
            test_index_path,
            total_docs,
            avg_doc_length,
            &doc_lengths,
            &pagerank,
            &doc_metadata,
            &sorted_terms,
            &inverted_index,
        )
        .expect("write_index failed");

        let fst_file = File::create(test_fst_path).expect("create fst failed");
        let fst_writer = BufWriter::new(fst_file);
        let mut builder = MapBuilder::new(fst_writer).expect("mapbuilder failed");
        for (term_idx, term) in sorted_terms.iter().enumerate() {
            builder
                .insert(term.as_bytes(), term_idx as u64)
                .expect("insert term");
        }
        builder.finish().expect("finish fst");

        let mmap_index = MmapIndex::open(test_index_path).expect("open index failed");
        let dictionary = TermDictionary::open(test_fst_path).expect("open dict failed");
        let pipeline = TokenizerPipeline::new();

        // Query: "alpha beta gamma" with top_k = 2
        let top2_res = search_combined_mmap(
            "alpha beta gamma",
            &mmap_index,
            &dictionary,
            &pipeline,
            1.0,
            DEFAULT_EPSILON,
            2,
        );
        assert_eq!(top2_res.len(), 2);
        assert_eq!(top2_res[0].0.hex_id, "doc0");
        assert_eq!(top2_res[1].0.hex_id, "doc1");
        assert!(top2_res[0].1 > top2_res[1].1);

        // Query with top_k = 6 -> all 6 docs returned in order
        let all_res = search_combined_mmap(
            "alpha beta gamma",
            &mmap_index,
            &dictionary,
            &pipeline,
            1.0,
            DEFAULT_EPSILON,
            6,
        );
        assert_eq!(all_res.len(), 6);
        assert_eq!(all_res[0].0.hex_id, "doc0");
        assert_eq!(all_res[1].0.hex_id, "doc1");

        // Top 2 scores match exactly between top_k=2 and top_k=6
        assert!((top2_res[0].1 - all_res[0].1).abs() < 1e-9);
        assert!((top2_res[1].1 - all_res[1].1).abs() < 1e-9);

        let _ = std::fs::remove_file(test_index_path);
        let _ = std::fs::remove_file(test_fst_path);
    }
}
