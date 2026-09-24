use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct PageRankConfig {
    pub damping: f64,
    pub max_iterations: usize,
    pub tolerance: f64,
}

impl Default for PageRankConfig {
    fn default() -> Self {
        Self {
            damping: 0.85,
            max_iterations: 100,
            tolerance: 1e-7,
        }
    }
}

pub fn resolve_url_to_id(link: &str, url_to_id: &HashMap<String, usize>) -> Option<usize> {
    if let Some(&id) = url_to_id.get(link) {
        return Some(id);
    }
    if let Some(stripped) = link.strip_suffix('/') {
        if let Some(&id) = url_to_id.get(stripped) {
            return Some(id);
        }
    } else {
        let with_slash = format!("{link}/");
        if let Some(&id) = url_to_id.get(&with_slash) {
            return Some(id);
        }
    }
    None
}

pub fn build_pagerank_graph(
    total_docs: usize,
    doc_links: &[Vec<String>],
    url_to_id: &HashMap<String, usize>,
) -> Vec<Vec<usize>> {
    let mut out_edges = Vec::with_capacity(total_docs);
    for (u, links) in doc_links.iter().enumerate() {
        let mut targets = HashSet::new();
        for link in links {
            if let Some(target_id) = resolve_url_to_id(link, url_to_id)
                && target_id != u
            {
                targets.insert(target_id);
            }
        }
        let mut target_vec: Vec<usize> = targets.into_iter().collect();
        target_vec.sort_unstable();
        out_edges.push(target_vec);
    }
    out_edges
}

pub fn power_iteration_pagerank(
    total_docs: usize,
    out_edges: &[Vec<usize>],
    config: &PageRankConfig,
) -> Vec<f64> {
    if total_docs == 0 {
        return Vec::new();
    }
    if total_docs == 1 {
        return vec![1.0];
    }

    let n = total_docs as f64;
    let damping = config.damping;
    let mut pr = vec![1.0 / n; total_docs];

    let out_degrees: Vec<usize> = out_edges.iter().map(|edges| edges.len()).collect();

    for _iter in 0..config.max_iterations {
        // Collect PageRank mass from dangling nodes (out_degree == 0)
        let dangling_sum: f64 = (0..total_docs)
            .filter(|&u| out_degrees[u] == 0)
            .map(|u| pr[u])
            .sum();

        let base_score = (1.0 - damping) / n + (damping * dangling_sum) / n;
        let mut next_pr = vec![base_score; total_docs];

        for u in 0..total_docs {
            let deg = out_degrees[u];
            if deg > 0 {
                let contribution = (damping * pr[u]) / (deg as f64);
                for &v in &out_edges[u] {
                    next_pr[v] += contribution;
                }
            }
        }

        let delta: f64 = pr
            .iter()
            .zip(next_pr.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();

        pr = next_pr;

        if delta < config.tolerance {
            break;
        }
    }

    pr
}

pub fn compute_pagerank(
    total_docs: usize,
    doc_links: &[Vec<String>],
    url_to_id: &HashMap<String, usize>,
    config: &PageRankConfig,
) -> Vec<f64> {
    let out_edges = build_pagerank_graph(total_docs, doc_links, url_to_id);
    power_iteration_pagerank(total_docs, &out_edges, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagerank_two_nodes_symmetric() {
        // 0 -> 1 and 1 -> 0
        let edges = vec![vec![1], vec![0]];
        let config = PageRankConfig::default();
        let pr = power_iteration_pagerank(2, &edges, &config);

        assert_eq!(pr.len(), 2);
        assert!((pr[0] - 0.5).abs() < 1e-6);
        assert!((pr[1] - 0.5).abs() < 1e-6);
        assert!((pr[0] + pr[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_pagerank_three_nodes_flow() {
        // 0 -> 1, 0 -> 2
        // 1 -> 2
        // 2 -> 0
        // Node 2 has highest incoming authority
        let edges = vec![vec![1, 2], vec![2], vec![0]];
        let config = PageRankConfig::default();
        let pr = power_iteration_pagerank(3, &edges, &config);

        assert_eq!(pr.len(), 3);
        let sum: f64 = pr.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(pr[2] > pr[0]);
        assert!(pr[2] > pr[1]);
    }

    #[test]
    fn test_pagerank_dangling_node_mass_preservation() {
        // 0 -> 1; 1 has no outgoing links (dangling)
        let edges = vec![vec![1], vec![]];
        let config = PageRankConfig::default();
        let pr = power_iteration_pagerank(2, &edges, &config);

        assert_eq!(pr.len(), 2);
        let sum: f64 = pr.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        // Node 1 receives incoming link from 0, so pr[1] > pr[0]
        assert!(pr[1] > pr[0]);
    }

    #[test]
    fn test_pagerank_empty_and_single_node() {
        let config = PageRankConfig::default();
        assert_eq!(power_iteration_pagerank(0, &[], &config), Vec::<f64>::new());
        assert_eq!(power_iteration_pagerank(1, &[vec![]], &config), vec![1.0]);
    }
}
