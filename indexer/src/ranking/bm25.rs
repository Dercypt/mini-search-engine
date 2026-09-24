use crate::tokenizer::TokenizerPipeline;

pub const DEFAULT_ALPHA: f64 = 0.7;
pub const DEFAULT_EPSILON: f64 = 1e-6;

#[inline]
pub fn final_score(bm25_score: f64, pagerank: f64, alpha: f64, epsilon: f64) -> f64 {
    alpha * bm25_score + (1.0 - alpha) * (pagerank + epsilon).ln()
}

#[inline]
pub fn compute_final_score(bm25_score: f64, pagerank: f64, alpha: f64, epsilon: f64) -> f64 {
    final_score(bm25_score, pagerank, alpha, epsilon)
}

#[inline]
pub fn bm25_idf(total_docs: f64, doc_freq: f64) -> f64 {
    ((total_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln()
}

#[inline]
pub fn bm25_tf_weight(tf: f64, doc_len: f64, avg_doc_len: f64, k1: f64, b: f64) -> f64 {
    if tf <= 0.0 {
        return 0.0;
    }
    let num = tf * (k1 + 1.0);
    let denom = tf + k1 * (1.0 - b + b * (doc_len / avg_doc_len));
    num / denom
}

#[inline]
pub fn bm25_score(
    total_docs: f64,
    doc_freq: f64,
    tf: f64,
    doc_len: f64,
    avg_doc_len: f64,
    k1: f64,
    b: f64,
) -> f64 {
    bm25_idf(total_docs, doc_freq) * bm25_tf_weight(tf, doc_len, avg_doc_len, k1, b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub phrases: Vec<Vec<String>>,
    pub unquoted_terms: Vec<String>,
}

impl ParsedQuery {
    pub fn all_terms(&self) -> Vec<String> {
        let mut terms = Vec::new();
        for phrase in &self.phrases {
            for term in phrase {
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

    let unquoted_terms = pipeline.tokenize(&unquoted_text);

    ParsedQuery {
        phrases,
        unquoted_terms,
    }
}

pub fn phrase_matches(positions_list: &[&[u32]]) -> bool {
    if positions_list.is_empty() {
        return true;
    }
    if positions_list.len() == 1 {
        return !positions_list[0].is_empty();
    }
    let mut current_starts: Vec<u32> = positions_list[0].to_vec();
    for (offset, next_positions) in positions_list.iter().skip(1).enumerate() {
        let expected_offset = (offset + 1) as u32;
        let mut next_starts = Vec::new();
        let mut i = 0;
        let mut j = 0;
        while i < current_starts.len() && j < next_positions.len() {
            let target = current_starts[i].saturating_add(expected_offset);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_idf_known_values() {
        // N = 100, n_q = 1: idf = ln((100 - 1 + 0.5)/(1 + 0.5) + 1.0) = ln(99.5/1.5 + 1.0)
        let idf_rare = bm25_idf(100.0, 1.0);
        let expected_rare = (99.5 / 1.5 + 1.0_f64).ln();
        assert!((idf_rare - expected_rare).abs() < 1e-10);
        assert!((idf_rare - 4.209655).abs() < 1e-5);

        // N = 100, n_q = 50: idf = ln((100 - 50 + 0.5)/(50 + 0.5) + 1.0) = ln(1.0 + 1.0) = ln(2.0)
        let idf_half = bm25_idf(100.0, 50.0);
        let expected_half = 2.0_f64.ln();
        assert!((idf_half - expected_half).abs() < 1e-10);
        assert!((idf_half - 0.693147).abs() < 1e-5);
    }

    #[test]
    fn test_bm25_idf_monotonicity_and_positivity() {
        let total_docs = 1000.0;
        // As document frequency increases, IDF must strictly decrease
        let idf_1 = bm25_idf(total_docs, 1.0);
        let idf_10 = bm25_idf(total_docs, 10.0);
        let idf_100 = bm25_idf(total_docs, 100.0);
        let idf_1000 = bm25_idf(total_docs, 1000.0);

        assert!(idf_1 > idf_10);
        assert!(idf_10 > idf_100);
        assert!(idf_100 > idf_1000);

        // Even when a term appears in every document (n_q = total_docs), IDF is strictly positive
        assert!(idf_1000 > 0.0);
    }

    #[test]
    fn test_bm25_tf_weight_properties() {
        let k1 = 1.5;
        let b = 0.75;
        let avg_doc_len = 100.0;

        // When tf == 0, weight must be 0
        assert_eq!(bm25_tf_weight(0.0, avg_doc_len, avg_doc_len, k1, b), 0.0);

        // When doc_len == avg_doc_len and tf == 1.0:
        // num = 1.0 * (k1 + 1.0) = 2.5
        // denom = 1.0 + k1 * (1 - b + b * 1) = 1.0 + 1.5 = 2.5
        // weight is exactly 1.0
        let weight_standard = bm25_tf_weight(1.0, avg_doc_len, avg_doc_len, k1, b);
        assert!((weight_standard - 1.0).abs() < 1e-10);

        // Monotonic increase with term frequency:
        let weight_tf1 = bm25_tf_weight(1.0, avg_doc_len, avg_doc_len, k1, b);
        let weight_tf2 = bm25_tf_weight(2.0, avg_doc_len, avg_doc_len, k1, b);
        let weight_tf5 = bm25_tf_weight(5.0, avg_doc_len, avg_doc_len, k1, b);
        let weight_tf100 = bm25_tf_weight(100.0, avg_doc_len, avg_doc_len, k1, b);

        assert!(weight_tf1 < weight_tf2);
        assert!(weight_tf2 < weight_tf5);
        assert!(weight_tf5 < weight_tf100);

        // Upper bounded by (k1 + 1.0) = 2.5
        assert!(weight_tf100 < 2.5);
    }

    #[test]
    fn test_bm25_document_length_penalty() {
        let k1 = 1.5;
        let b = 0.75;
        let avg_doc_len = 100.0;
        let tf = 2.0;

        let short_doc_weight = bm25_tf_weight(tf, 50.0, avg_doc_len, k1, b);
        let normal_doc_weight = bm25_tf_weight(tf, 100.0, avg_doc_len, k1, b);
        let long_doc_weight = bm25_tf_weight(tf, 200.0, avg_doc_len, k1, b);

        // Shorter documents receive higher term weight than longer documents with the same TF
        assert!(short_doc_weight > normal_doc_weight);
        assert!(normal_doc_weight > long_doc_weight);
    }

    #[test]
    fn test_bm25_score_computation() {
        let total_docs = 500.0;
        let doc_freq = 10.0;
        let tf = 3.0;
        let doc_len = 120.0;
        let avg_doc_len = 100.0;
        let k1 = 1.5;
        let b = 0.75;

        let idf = bm25_idf(total_docs, doc_freq);
        let tf_weight = bm25_tf_weight(tf, doc_len, avg_doc_len, k1, b);
        let score = bm25_score(total_docs, doc_freq, tf, doc_len, avg_doc_len, k1, b);

        assert_eq!(score, idf * tf_weight);
        assert!(score > 0.0);
    }

    #[test]
    fn test_final_score_formula() {
        let bm25: f64 = 4.0;
        let pr: f64 = 0.05;
        let alpha: f64 = 0.7;
        let epsilon: f64 = 1e-6;

        let expected = alpha * bm25 + (1.0 - alpha) * (pr + epsilon).ln();
        let computed = final_score(bm25, pr, alpha, epsilon);

        assert!((computed - expected).abs() < 1e-10);
    }

    #[test]
    fn test_final_score_alpha_bounds() {
        let bm25: f64 = 5.5;
        let pr: f64 = 0.02;
        let epsilon: f64 = 1e-6;

        // When alpha = 1.0, score must equal pure BM25
        let score_pure_bm25 = final_score(bm25, pr, 1.0, epsilon);
        assert!((score_pure_bm25 - bm25).abs() < 1e-10);

        // When alpha = 0.0, score must equal pure log(PageRank + epsilon)
        let score_pure_pr = final_score(bm25, pr, 0.0, epsilon);
        assert!((score_pure_pr - (pr + epsilon).ln()).abs() < 1e-10);
    }

    #[test]
    fn test_parse_query_variations() {
        let pipeline = TokenizerPipeline::new();

        // Plain unquoted query
        let q1 = parse_query("distributed systems", &pipeline);
        assert!(q1.phrases.is_empty());
        assert_eq!(q1.unquoted_terms, vec!["distribut", "system"]);

        // Pure quoted query
        let q2 = parse_query("\"distributed systems\"", &pipeline);
        assert_eq!(q2.phrases, vec![vec!["distribut", "system"]]);
        assert!(q2.unquoted_terms.is_empty());

        // Mixed quoted and unquoted
        let q3 = parse_query("rust \"distributed systems\" fast", &pipeline);
        assert_eq!(q3.phrases, vec![vec!["distribut", "system"]]);
        assert_eq!(q3.unquoted_terms, vec!["rust", "fast"]);

        // Multiple quoted phrases
        let q4 = parse_query("\"distributed systems\" \"fault tolerance\"", &pipeline);
        assert_eq!(
            q4.phrases,
            vec![vec!["distribut", "system"], vec!["fault", "toler"]]
        );
        assert!(q4.unquoted_terms.is_empty());

        // Unclosed quote at end
        let q5 = parse_query("\"distributed systems", &pipeline);
        assert_eq!(q5.phrases, vec![vec!["distribut", "system"]]);
        assert!(q5.unquoted_terms.is_empty());
    }

    #[test]
    fn test_phrase_matches_basic() {
        // Single term
        assert!(phrase_matches(&[&[1, 5, 10]]));
        assert!(!phrase_matches(&[&[]]));

        // Two terms consecutive: pos 4 in first, pos 5 in second
        let pos1 = [2, 4, 10];
        let pos2 = [5, 20];
        assert!(phrase_matches(&[&pos1, &pos2]));

        // Two terms non-consecutive
        let pos1_fail = [2, 4, 10];
        let pos2_fail = [7, 20];
        assert!(!phrase_matches(&[&pos1_fail, &pos2_fail]));

        // Two terms reverse order
        let pos1_rev = [5];
        let pos2_rev = [4];
        assert!(!phrase_matches(&[&pos1_rev, &pos2_rev]));

        // Three terms consecutive: 10, 11, 12
        let p1 = [1, 10, 50];
        let p2 = [2, 11, 60];
        let p3 = [3, 12, 70];
        assert!(phrase_matches(&[&p1, &p2, &p3]));

        // Three terms where third term misses
        let p1_m = [10, 50];
        let p2_m = [11, 60];
        let p3_m_fail = [13, 70];
        assert!(!phrase_matches(&[&p1_m, &p2_m, &p3_m_fail]));
    }
}
