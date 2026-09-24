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
    fn test_final_score_computation() {
        let bm25: f64 = 6.0;
        let pr: f64 = 0.04;
        let alpha: f64 = 0.7;
        let epsilon: f64 = 1e-6;

        let expected = alpha * bm25 + (1.0 - alpha) * (pr + epsilon).ln();
        let computed = final_score(bm25, pr, alpha, epsilon);

        assert!((computed - expected).abs() < 1e-10);
    }

    #[test]
    fn test_final_score_alpha_extremes() {
        let bm25: f64 = 4.2;
        let pr: f64 = 0.01;
        let epsilon: f64 = 1e-6;

        assert!((final_score(bm25, pr, 1.0, epsilon) - bm25).abs() < 1e-10);
        assert!((final_score(bm25, pr, 0.0, epsilon) - (pr + epsilon).ln()).abs() < 1e-10);
    }

    #[test]
    fn test_bm25_idf_known_values() {
        let idf_rare = bm25_idf(100.0, 1.0);
        let expected_rare = (99.5 / 1.5 + 1.0_f64).ln();
        assert!((idf_rare - expected_rare).abs() < 1e-10);
        assert!((idf_rare - 4.209655).abs() < 1e-5);

        let idf_half = bm25_idf(100.0, 50.0);
        assert!((idf_half - 2.0_f64.ln()).abs() < 1e-10);
    }

    #[test]
    fn test_bm25_tf_weight_properties() {
        let k1 = 1.5;
        let b = 0.75;
        let avg_doc_len = 80.0;

        assert_eq!(bm25_tf_weight(0.0, avg_doc_len, avg_doc_len, k1, b), 0.0);

        let weight_1 = bm25_tf_weight(1.0, avg_doc_len, avg_doc_len, k1, b);
        assert!((weight_1 - 1.0).abs() < 1e-10);

        let weight_2 = bm25_tf_weight(2.0, avg_doc_len, avg_doc_len, k1, b);
        let weight_5 = bm25_tf_weight(5.0, avg_doc_len, avg_doc_len, k1, b);
        assert!(weight_1 < weight_2);
        assert!(weight_2 < weight_5);
    }

    #[test]
    fn test_bm25_document_length_penalty() {
        let k1 = 1.5;
        let b = 0.75;
        let avg_doc_len = 100.0;
        let tf = 2.0;

        let short_doc = bm25_tf_weight(tf, 40.0, avg_doc_len, k1, b);
        let long_doc = bm25_tf_weight(tf, 250.0, avg_doc_len, k1, b);
        assert!(short_doc > long_doc);
    }

    #[test]
    fn test_bm25_score_computation() {
        let total_docs = 1000.0;
        let doc_freq = 5.0;
        let tf = 2.0;
        let doc_len = 100.0;
        let avg_doc_len = 100.0;
        let k1 = 1.5;
        let b = 0.75;

        let score = bm25_score(total_docs, doc_freq, tf, doc_len, avg_doc_len, k1, b);
        let expected =
            bm25_idf(total_docs, doc_freq) * bm25_tf_weight(tf, doc_len, avg_doc_len, k1, b);
        assert_eq!(score, expected);
        assert!(score > 0.0);
    }

    #[test]
    fn test_phrase_matches_search_api() {
        // Single term match
        assert!(phrase_matches(&[&[1, 5, 10]]));
        assert!(!phrase_matches(&[&[]]));

        // Consecutive phrase match
        let pos1 = [2, 4, 10];
        let pos2 = [5, 20];
        assert!(phrase_matches(&[&pos1, &pos2]));

        // Non-consecutive positions
        let pos1_fail = [2, 4, 10];
        let pos2_fail = [7, 20];
        assert!(!phrase_matches(&[&pos1_fail, &pos2_fail]));

        // Three terms consecutive match
        let p1 = [10, 50];
        let p2 = [11, 60];
        let p3 = [12, 70];
        assert!(phrase_matches(&[&p1, &p2, &p3]));

        // Three terms where third term misses
        let p3_fail = [14, 70];
        assert!(!phrase_matches(&[&p1, &p2, &p3_fail]));
    }

    #[test]
    fn test_parse_query_search_api() {
        let pipeline = TokenizerPipeline::new();

        // Plain unquoted
        let q1 = parse_query("distributed systems", &pipeline);
        assert!(q1.phrases.is_empty());
        assert_eq!(q1.unquoted_terms, vec!["distribut", "system"]);

        // Pure quoted
        let q2 = parse_query("\"distributed systems\"", &pipeline);
        assert_eq!(q2.phrases, vec![vec!["distribut", "system"]]);
        assert!(q2.unquoted_terms.is_empty());

        // Mixed quoted
        let q3 = parse_query("rust \"distributed systems\" query", &pipeline);
        assert_eq!(q3.phrases, vec![vec!["distribut", "system"]]);
        assert_eq!(q3.unquoted_terms, vec!["rust", "queri"]);

        // Unclosed quote
        let q4 = parse_query("\"distributed systems", &pipeline);
        assert_eq!(q4.phrases, vec![vec!["distribut", "system"]]);
        assert!(q4.unquoted_terms.is_empty());
    }

    #[test]
    fn test_quoted_query_phrase_filtering() {
        let pipeline = TokenizerPipeline::new();

        let doc1_distribut = vec![2];
        let doc1_system = vec![3]; // consecutive: pos 2 + 1 = 3 -> MATCH

        let doc2_distribut = vec![2];
        let doc2_system = vec![10]; // non-consecutive -> NO MATCH

        let doc3_distribut = vec![15];
        let doc3_system = vec![14]; // reversed -> NO MATCH for "distributed systems", MATCH for "systems distributed"

        // Phrase "distributed systems"
        assert!(phrase_matches(&[&doc1_distribut, &doc1_system]));
        assert!(!phrase_matches(&[&doc2_distribut, &doc2_system]));
        assert!(!phrase_matches(&[&doc3_distribut, &doc3_system]));

        // Phrase "systems distributed"
        assert!(!phrase_matches(&[&doc1_system, &doc1_distribut]));
        assert!(!phrase_matches(&[&doc2_system, &doc2_distribut]));
        assert!(phrase_matches(&[&doc3_system, &doc3_distribut]));

        // Check query parsing
        let parsed = parse_query("\"distributed systems\"", &pipeline);
        assert_eq!(parsed.phrases, vec![vec!["distribut", "system"]]);
        assert_eq!(parsed.all_terms(), vec!["distribut", "system"]);
    }
}
