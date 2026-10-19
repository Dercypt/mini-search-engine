use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub struct TokenizerPipeline {
    stemmer: Arc<Stemmer>,
    stop_words: Arc<HashSet<&'static str>>,
    regex: Regex,
}

impl Default for TokenizerPipeline {
    fn default() -> Self {
        Self::new()
    }
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
            stemmer: Arc::new(Stemmer::create(Algorithm::English)),
            stop_words: Arc::new(stop_words),
            regex: Regex::new(r"[^a-zA-Z0-9\s]+").unwrap(),
        }
    }

    pub fn tokenize(&self, text: &str) -> Vec<(usize, String)> {
        let cleaned = self.regex.replace_all(text, " ").to_lowercase();
        cleaned
            .split_whitespace()
            .enumerate()
            .filter(|(_idx, token)| token.len() > 1 && !self.stop_words.contains(token))
            .map(|(idx, token)| (idx, self.stemmer.stem(token).to_string()))
            .collect()
    }

    pub fn word_count(&self, text: &str) -> usize {
        self.regex.replace_all(text, " ").split_whitespace().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_preserves_original_word_positions() {
        let pipeline = TokenizerPipeline::new();
        let tokens = pipeline.tokenize("search the web with an engine");
        assert_eq!(
            tokens,
            vec![
                (0, "search".to_string()),
                (2, "web".to_string()),
                (5, "engin".to_string()),
            ]
        );
    }

    #[test]
    fn test_tokenize_stop_word_only_phrase_is_empty() {
        let pipeline = TokenizerPipeline::new();
        let tokens = pipeline.tokenize("to be or not to be");
        assert!(tokens.is_empty());
    }
}
