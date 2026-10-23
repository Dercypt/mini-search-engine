use crate::tokenizer::TokenizerPipeline;

pub const DEFAULT_ALPHA: f64 = 0.85;
pub const DEFAULT_BETA: f64 = 1000.0;

#[inline]
pub fn bm25_idf(total_docs: f64, doc_freq: f64) -> f64 {
    ((total_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln()
}

#[inline]
pub fn bm25_tf_weight(tf: f64, doc_len: f64, avg_doc_len: f64, k1: f64, b: f64) -> f64 {
    let b_factor = if avg_doc_len > 0.0 {
        1.0 - b + b * (doc_len / avg_doc_len)
    } else {
        1.0
    };
    (tf * (k1 + 1.0)) / (tf + k1 * b_factor)
}

#[inline]
pub fn final_score(bm25: f64, pagerank: f64, alpha: f64, beta: f64) -> f64 {
    let alpha_clamped = alpha.clamp(0.0, 1.0);
    let bm25_part = alpha_clamped * bm25;
    let pr_part = if alpha_clamped < 1.0 {
        (1.0 - alpha_clamped) * (1.0 + beta * pagerank).ln()
    } else {
        0.0
    };
    bm25_part + pr_part
}

/// Computes the theoretical upper bound of the PageRank contribution to `final_score`.
///
/// Since PageRank values are normalized such that $PR \le 1.0$, the PageRank score
/// component $(1 - \alpha) \ln(1 + \beta \cdot PR)$ is bounded by $(1 - \alpha) \ln(1 + \beta)$.
/// When $\alpha \ge 1.0$, PageRank is disabled and the bound is $0.0$.
#[inline]
pub fn pagerank_score_upper_bound(alpha: f64, beta: f64) -> f64 {
    let alpha_clamped = alpha.clamp(0.0, 1.0);
    if alpha_clamped < 1.0 {
        (1.0 - alpha_clamped) * (1.0 + beta.max(0.0)).ln()
    } else {
        0.0
    }
}

#[inline]
pub fn bm25_score(
    tf: f64,
    doc_len: f64,
    avg_doc_len: f64,
    total_docs: f64,
    doc_freq: f64,
    k1: f64,
    b: f64,
) -> f64 {
    bm25_idf(total_docs, doc_freq) * bm25_tf_weight(tf, doc_len, avg_doc_len, k1, b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub phrases: Vec<Vec<(usize, String)>>,
    pub unquoted_terms: Vec<String>,
}

impl ParsedQuery {
    pub fn all_terms(&self) -> Vec<String> {
        let mut terms = Vec::new();
        for phrase in &self.phrases {
            for (_pos, term) in phrase {
                if !terms.contains(term) {
                    terms.push(term.clone());
                }
            }
        }
        for term in &self.unquoted_terms {
            if !terms.contains(term) {
                terms.push(term.clone());
            }
        }
        terms
    }
}

pub fn parse_query(query: &str, pipeline: &TokenizerPipeline) -> ParsedQuery {
    let mut phrases = Vec::new();
    let mut unquoted_text = String::new();

    let mut in_quote = false;
    let mut current_phrase = String::new();

    for ch in query.chars() {
        if ch == '"' {
            if in_quote {
                let tokens = pipeline.tokenize(&current_phrase);
                if !tokens.is_empty() {
                    phrases.push(tokens);
                }
                current_phrase.clear();
                in_quote = false;
            } else {
                in_quote = true;
            }
        } else if in_quote {
            current_phrase.push(ch);
        } else {
            unquoted_text.push(ch);
        }
    }

    if in_quote && !current_phrase.is_empty() {
        let tokens = pipeline.tokenize(&current_phrase);
        if !tokens.is_empty() {
            phrases.push(tokens);
        }
    }

    let unquoted_terms = pipeline
        .tokenize(&unquoted_text)
        .into_iter()
        .map(|(_, term)| term)
        .collect();

    ParsedQuery {
        phrases,
        unquoted_terms,
    }
}

pub fn phrase_matches_with_offsets(offsets: &[usize], positions_list: &[&[u32]]) -> bool {
    if positions_list.is_empty() {
        return true;
    }
    if positions_list.len() == 1 {
        return !positions_list[0].is_empty();
    }
    if offsets.len() != positions_list.len() {
        return false;
    }
    let base_offset = offsets[0];
    let mut current_starts: Vec<u32> = positions_list[0].to_vec();
    for (k, next_positions) in positions_list.iter().skip(1).enumerate() {
        let word_offset = offsets[k + 1];
        let expected_delta = word_offset.saturating_sub(base_offset) as u32;
        let mut next_starts = Vec::new();
        let mut i = 0;
        let mut j = 0;
        while i < current_starts.len() && j < next_positions.len() {
            let target = current_starts[i].saturating_add(expected_delta);
            if next_positions[j] == target {
                next_starts.push(current_starts[i]);
                i += 1;
                j += 1;
            } else if next_positions[j] < target {
                j += 1;
            } else {
                i += 1;
            }
        }
        current_starts = next_starts;
        if current_starts.is_empty() {
            return false;
        }
    }
    !current_starts.is_empty()
}

pub fn phrase_matches(positions_list: &[&[u32]]) -> bool {
    let offsets: Vec<usize> = (0..positions_list.len()).collect();
    phrase_matches_with_offsets(&offsets, positions_list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_idf_known_values() {
        let n = 1000.0;

        let idf_rare = bm25_idf(n, 1.0);
        let expected_rare = (((1000.0 - 1.0 + 0.5) / (1.0 + 0.5) + 1.0) as f64).ln();
        assert!((idf_rare - expected_rare).abs() < 1e-9);

        let idf_common = bm25_idf(n, 500.0);
        let expected_common = (((1000.0 - 500.0 + 0.5) / (500.0 + 0.5) + 1.0) as f64).ln();
        assert!((idf_common - expected_common).abs() < 1e-9);

        let idf_universal = bm25_idf(n, 1000.0);
        let expected_universal = (((1000.0 - 1000.0 + 0.5) / (1000.0 + 0.5) + 1.0) as f64).ln();
        assert!((idf_universal - expected_universal).abs() < 1e-9);
    }

    #[test]
    fn test_bm25_idf_monotonicity_and_positivity() {
        let n = 10_000.0;
        let mut prev_idf = f64::INFINITY;

        for df in [1.0, 5.0, 50.0, 500.0, 2000.0, 5000.0, 9999.0, 10_000.0] {
            let idf = bm25_idf(n, df);
            assert!(
                idf > 0.0,
                "IDF must be strictly positive for df = {}, got {}",
                df,
                idf
            );
            assert!(
                idf <= prev_idf,
                "IDF must monotonically decrease with df: {} > {}",
                idf,
                prev_idf
            );
            prev_idf = idf;
        }
    }

    #[test]
    fn test_bm25_tf_weight_properties() {
        let k1 = 1.5;
        let b = 0.75;
        let doc_len = 100.0;
        let avg_len = 100.0;

        let tf0 = bm25_tf_weight(0.0, doc_len, avg_len, k1, b);
        assert_eq!(tf0, 0.0);

        let mut prev = 0.0;
        for tf in [1.0, 2.0, 5.0, 10.0, 50.0, 100.0, 1000.0] {
            let curr = bm25_tf_weight(tf, doc_len, avg_len, k1, b);
            assert!(
                curr > prev,
                "TF weight must increase monotonically: {} <= {}",
                curr,
                prev
            );
            assert!(
                curr < k1 + 1.0,
                "TF weight must be bounded by k1 + 1 (2.5), got {}",
                curr
            );
            prev = curr;
        }
    }

    #[test]
    fn test_bm25_document_length_penalty() {
        let k1 = 1.5;
        let b = 0.75;
        let avg_len = 100.0;
        let tf = 5.0;

        let short_doc_score = bm25_tf_weight(tf, 50.0, avg_len, k1, b);
        let avg_doc_score = bm25_tf_weight(tf, 100.0, avg_len, k1, b);
        let long_doc_score = bm25_tf_weight(tf, 500.0, avg_len, k1, b);

        assert!(short_doc_score > avg_doc_score);
        assert!(avg_doc_score > long_doc_score);
    }

    #[test]
    fn test_bm25_score_computation() {
        let score = bm25_score(2.0, 150.0, 100.0, 500.0, 20.0, 1.5, 0.75);
        assert!(score > 0.0);
        assert!(score.is_finite());
    }

    #[test]
    fn test_final_score_formula() {
        let bm25 = 2.5;
        let pr = 0.005;
        let alpha = 0.7;
        let beta = 1000.0;

        let score = final_score(bm25, pr, alpha, beta);
        let expected = 0.7 * 2.5 + 0.3 * (1.0 + 1000.0 * 0.005_f64).ln();
        assert!((score - expected).abs() < 1e-9);
    }

    #[test]
    fn test_final_score_alpha_bounds() {
        let s_bm25_only = final_score(3.0, 0.5, 1.0, DEFAULT_BETA);
        assert!((s_bm25_only - 3.0).abs() < 1e-6);

        let s_pr_only = final_score(3.0, 0.5, 0.0, DEFAULT_BETA);
        let expected_pr = (1.0 + 0.5 * DEFAULT_BETA).ln();
        assert!((s_pr_only - expected_pr).abs() < 1e-6);
    }

    #[test]
    fn test_final_score_positivity_and_monotonicity() {
        let s1 = final_score(0.0, 0.0, 0.7, DEFAULT_BETA);
        assert_eq!(s1, 0.0);

        let s2 = final_score(1.0, 0.0, 0.7, DEFAULT_BETA);
        assert!(s2 > 0.0);

        let s3 = final_score(0.0, 0.01, 0.7, DEFAULT_BETA);
        assert!(s3 > 0.0);

        let s_low = final_score(3.0, 0.001, 0.7, DEFAULT_BETA);
        let s_high = final_score(3.0, 0.01, 0.7, DEFAULT_BETA);
        assert!(s_high > s_low);
    }

    #[test]
    fn test_pagerank_score_upper_bound() {
        let alpha = DEFAULT_ALPHA; // 0.85
        let beta = DEFAULT_BETA; // 1000.0

        let bound = pagerank_score_upper_bound(alpha, beta);
        let expected = (1.0 - 0.85) * (1001.0_f64).ln();
        assert!((bound - expected).abs() < 1e-9);

        // Bound must strictly upper-bound any final_score PageRank component for PR in [0, 1]
        for pr_raw in [0.0, 0.0001, 0.01, 0.5, 0.99, 1.0] {
            let pr_component = final_score(0.0, pr_raw, alpha, beta);
            assert!(pr_component <= bound + 1e-12);
        }

        // When alpha = 1.0, bound is 0.0
        assert_eq!(pagerank_score_upper_bound(1.0, beta), 0.0);
        // When alpha > 1.0 (clamped), bound is 0.0
        assert_eq!(pagerank_score_upper_bound(1.5, beta), 0.0);
    }

    #[test]
    fn test_parse_query_variations() {
        let pipeline = TokenizerPipeline::new();

        let q1 = parse_query("distributed systems", &pipeline);
        assert!(q1.phrases.is_empty());
        assert_eq!(q1.unquoted_terms, vec!["distribut", "system"]);

        let q2 = parse_query("\"distributed systems\"", &pipeline);
        assert_eq!(
            q2.phrases,
            vec![vec![
                (0, "distribut".to_string()),
                (1, "system".to_string()),
            ]]
        );
        assert!(q2.unquoted_terms.is_empty());

        let q3 = parse_query("rust \"distributed systems\" fast", &pipeline);
        assert_eq!(
            q3.phrases,
            vec![vec![
                (0, "distribut".to_string()),
                (1, "system".to_string()),
            ]]
        );
        assert_eq!(q3.unquoted_terms, vec!["rust", "fast"]);

        let q4 = parse_query("\"distributed systems\" \"fault tolerance\"", &pipeline);
        assert_eq!(
            q4.phrases,
            vec![
                vec![(0, "distribut".to_string()), (1, "system".to_string())],
                vec![(0, "fault".to_string()), (1, "toler".to_string())]
            ]
        );
        assert!(q4.unquoted_terms.is_empty());

        let q5 = parse_query("\"distributed systems", &pipeline);
        assert_eq!(
            q5.phrases,
            vec![vec![
                (0, "distribut".to_string()),
                (1, "system".to_string()),
            ]]
        );
        assert!(q5.unquoted_terms.is_empty());
    }

    #[test]
    fn test_phrase_matches_basic() {
        assert!(phrase_matches(&[&[1, 5, 10]]));
        assert!(!phrase_matches(&[&[]]));

        let pos1 = [2, 4, 10];
        let pos2 = [5, 20];
        assert!(phrase_matches(&[&pos1, &pos2]));

        let pos1_fail = [2, 4, 10];
        let pos2_fail = [7, 20];
        assert!(!phrase_matches(&[&pos1_fail, &pos2_fail]));

        let pos1_rev = [5];
        let pos2_rev = [4];
        assert!(!phrase_matches(&[&pos1_rev, &pos2_rev]));

        let p1 = [1, 10, 50];
        let p2 = [2, 11, 60];
        let p3 = [3, 12, 70];
        assert!(phrase_matches(&[&p1, &p2, &p3]));

        let p1_m = [10, 50];
        let p2_m = [11, 60];
        let p3_m_fail = [13, 70];
        assert!(!phrase_matches(&[&p1_m, &p2_m, &p3_m_fail]));
    }

    #[test]
    fn test_phrase_matches_with_stop_word_offsets() {
        let search_pos = [0];
        let web_pos = [2];
        let engine_pos = [5];

        assert!(phrase_matches_with_offsets(
            &[0, 2],
            &[&search_pos, &web_pos]
        ));

        assert!(!phrase_matches_with_offsets(
            &[0, 1],
            &[&search_pos, &engine_pos]
        ));
    }
}
