use aho_corasick::AhoCorasick;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
};
use fst::automaton::{Levenshtein, Str};
use fst::{Automaton, IntoStreamer, Map, MapBuilder, Streamer};
use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufWriter;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ============================================================================
// VByte Compression & Postings Iterator
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    pub term_frequency: u32,
}

#[inline]
pub fn decode_vbyte(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let mut result = 0u32;
    let mut shift = 0;
    while *offset < bytes.len() {
        if shift > 28 {
            return None;
        }
        let byte = bytes[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            return Some(result);
        }
        shift += 7;
    }
    None
}

pub struct PostingsIterator<'a> {
    slice: &'a [u8],
    offset: usize,
    last_doc_id: u32,
    remaining: usize,
    index: usize,
}

impl<'a> PostingsIterator<'a> {
    pub fn new(slice: &'a [u8], doc_freq: usize) -> Self {
        Self {
            slice,
            offset: 0,
            last_doc_id: 0,
            remaining: doc_freq,
            index: 0,
        }
    }
}

impl<'a> Iterator for PostingsIterator<'a> {
    type Item = Posting;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let delta = decode_vbyte(self.slice, &mut self.offset)?;
        let doc_id = if self.index == 0 {
            delta
        } else {
            self.last_doc_id + delta
        };
        self.last_doc_id = doc_id;
        self.index += 1;
        self.remaining -= 1;
        let term_frequency = decode_vbyte(self.slice, &mut self.offset).unwrap_or(1);
        Some(Posting {
            doc_id,
            term_frequency,
        })
    }
}

// ============================================================================
// Memory-Mapped Document Store (documents.bin)
// ============================================================================

pub struct DocRecordRef<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub links: Vec<&'a str>,
}

pub struct MmapDocStore {
    mmap: memmap2::Mmap,
    doc_count: u32,
    index_offset: usize,
}

impl MmapDocStore {
    pub fn open<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        if mmap.len() < 64 || &mmap[0..8] != b"MSEDOC01" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid document store magic header",
            ));
        }
        let doc_count = u32::from_le_bytes(mmap[8..12].try_into().unwrap());
        let index_offset = u64::from_le_bytes(mmap[12..20].try_into().unwrap()) as usize;
        Ok(Self {
            mmap,
            doc_count,
            index_offset,
        })
    }

    #[inline]
    pub fn doc_count(&self) -> u32 {
        self.doc_count
    }

    pub fn get_doc_slice(&self, doc_id: u32) -> Option<&[u8]> {
        if doc_id >= self.doc_count {
            return None;
        }
        let entry_offset = self.index_offset + (doc_id as usize) * 12;
        if entry_offset + 12 > self.mmap.len() {
            return None;
        }
        let offset = u64::from_le_bytes(
            self.mmap[entry_offset..entry_offset + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        let len = u32::from_le_bytes(
            self.mmap[entry_offset + 8..entry_offset + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        if offset + len > self.mmap.len() {
            return None;
        }
        Some(&self.mmap[offset..offset + len])
    }

    pub fn get_doc(&self, doc_id: u32) -> Option<DocRecordRef<'_>> {
        let slice = self.get_doc_slice(doc_id)?;
        let mut pos = 0;
        if slice.len() < 2 {
            return None;
        }
        let id_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        if pos + id_len > slice.len() {
            return None;
        }
        let id = std::str::from_utf8(&slice[pos..pos + id_len]).ok()?;
        pos += id_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let url_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        if pos + url_len > slice.len() {
            return None;
        }
        let url = std::str::from_utf8(&slice[pos..pos + url_len]).ok()?;
        pos += url_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let title_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        if pos + title_len > slice.len() {
            return None;
        }
        let title = std::str::from_utf8(&slice[pos..pos + title_len]).ok()?;
        pos += title_len;

        if pos + 4 > slice.len() {
            return None;
        }
        let content_len =
            u32::from_le_bytes([slice[pos], slice[pos + 1], slice[pos + 2], slice[pos + 3]])
                as usize;
        pos += 4;
        if pos + content_len > slice.len() {
            return None;
        }
        let content = std::str::from_utf8(&slice[pos..pos + content_len]).ok()?;
        pos += content_len;

        let mut links = Vec::new();
        if pos + 4 <= slice.len() {
            let links_count =
                u32::from_le_bytes([slice[pos], slice[pos + 1], slice[pos + 2], slice[pos + 3]])
                    as usize;
            pos += 4;
            links.reserve(links_count);
            for _ in 0..links_count {
                if pos + 2 > slice.len() {
                    break;
                }
                let link_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
                pos += 2;
                if pos + link_len > slice.len() {
                    break;
                }
                if let Ok(link_str) = std::str::from_utf8(&slice[pos..pos + link_len]) {
                    links.push(link_str);
                }
                pos += link_len;
            }
        }

        Some(DocRecordRef {
            id,
            url,
            title,
            content,
            links,
        })
    }

    pub fn get_content(&self, doc_id: u32) -> Option<&str> {
        self.get_doc(doc_id).map(|d| d.content)
    }
}

// ============================================================================
// Memory-Mapped Binary Index (index.bin)
// ============================================================================

pub struct TermEntry {
    pub postings_offset: u64,
    pub postings_len: u32,
    pub doc_freq: u32,
}

pub struct DocMetaRef<'a> {
    pub hex_id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub pagerank: f64,
}

pub struct MmapIndex {
    mmap: memmap2::Mmap,
    pub total_docs: u32,
    pub avg_doc_length: f64,
    pub num_terms: u32,
    doc_lengths_offset: usize,
    pagerank_offset: usize,
    doc_meta_index_offset: usize,
    doc_meta_data_offset: usize,
    terms_table_offset: usize,
    terms_strings_offset: usize,
    postings_offset: usize,
}

impl MmapIndex {
    pub fn open<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        if mmap.len() < 128 || &mmap[0..8] != b"MSEIDX01" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid index magic header",
            ));
        }
        let total_docs = u32::from_le_bytes(mmap[8..12].try_into().unwrap());
        let avg_doc_length = f64::from_le_bytes(mmap[12..20].try_into().unwrap());
        let doc_lengths_offset = u64::from_le_bytes(mmap[20..28].try_into().unwrap()) as usize;
        let doc_meta_index_offset = u64::from_le_bytes(mmap[28..36].try_into().unwrap()) as usize;
        let doc_meta_data_offset = u64::from_le_bytes(mmap[36..44].try_into().unwrap()) as usize;
        let terms_table_offset = u64::from_le_bytes(mmap[44..52].try_into().unwrap()) as usize;
        let num_terms = u32::from_le_bytes(mmap[52..56].try_into().unwrap());
        let terms_strings_offset = u64::from_le_bytes(mmap[56..64].try_into().unwrap()) as usize;
        let postings_offset = u64::from_le_bytes(mmap[64..72].try_into().unwrap()) as usize;
        let pagerank_offset = if mmap.len() >= 88 {
            u64::from_le_bytes(mmap[80..88].try_into().unwrap()) as usize
        } else {
            0
        };

        Ok(Self {
            mmap,
            total_docs,
            avg_doc_length,
            num_terms,
            doc_lengths_offset,
            pagerank_offset,
            doc_meta_index_offset,
            doc_meta_data_offset,
            terms_table_offset,
            terms_strings_offset,
            postings_offset,
        })
    }

    #[inline]
    pub fn get_doc_length(&self, doc_id: u32) -> u32 {
        if doc_id >= self.total_docs {
            return 0;
        }
        let off = self.doc_lengths_offset + (doc_id as usize) * 4;
        u32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap())
    }

    #[inline]
    pub fn get_pagerank(&self, doc_id: u32) -> f64 {
        if doc_id >= self.total_docs || self.pagerank_offset == 0 {
            return if self.total_docs > 0 {
                1.0 / (self.total_docs as f64)
            } else {
                0.0
            };
        }
        let off = self.pagerank_offset + (doc_id as usize) * 8;
        if off + 8 > self.mmap.len() {
            return if self.total_docs > 0 {
                1.0 / (self.total_docs as f64)
            } else {
                0.0
            };
        }
        f64::from_le_bytes(self.mmap[off..off + 8].try_into().unwrap())
    }

    pub fn get_doc_meta(&self, doc_id: u32) -> Option<DocMetaRef<'_>> {
        if doc_id >= self.total_docs {
            return None;
        }
        let idx_off = self.doc_meta_index_offset + (doc_id as usize) * 8;
        let meta_rel_start =
            u64::from_le_bytes(self.mmap[idx_off..idx_off + 8].try_into().unwrap()) as usize;
        let meta_rel_end =
            u64::from_le_bytes(self.mmap[idx_off + 8..idx_off + 16].try_into().unwrap()) as usize;

        let start = self.doc_meta_data_offset + meta_rel_start;
        let end = self.doc_meta_data_offset + meta_rel_end;
        if end > self.mmap.len() || start >= end {
            return None;
        }
        let slice = &self.mmap[start..end];
        let mut pos = 0;
        if slice.len() < 2 {
            return None;
        }
        let hex_id_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        let hex_id = std::str::from_utf8(&slice[pos..pos + hex_id_len]).ok()?;
        pos += hex_id_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let url_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        let url = std::str::from_utf8(&slice[pos..pos + url_len]).ok()?;
        pos += url_len;

        if pos + 2 > slice.len() {
            return None;
        }
        let title_len = u16::from_le_bytes([slice[pos], slice[pos + 1]]) as usize;
        pos += 2;
        let title = std::str::from_utf8(&slice[pos..pos + title_len]).ok()?;

        let pagerank = self.get_pagerank(doc_id);

        Some(DocMetaRef {
            hex_id,
            url,
            title,
            pagerank,
        })
    }

    #[inline]
    pub fn get_term_entry(&self, term_idx: usize) -> Option<TermEntry> {
        if term_idx >= self.num_terms as usize {
            return None;
        }
        let off = self.terms_table_offset + term_idx * 16;
        if off + 16 > self.mmap.len() {
            return None;
        }
        let postings_offset = u64::from_le_bytes(self.mmap[off..off + 8].try_into().unwrap());
        let postings_len = u32::from_le_bytes(self.mmap[off + 8..off + 12].try_into().unwrap());
        let doc_freq = u32::from_le_bytes(self.mmap[off + 12..off + 16].try_into().unwrap());
        Some(TermEntry {
            postings_offset,
            postings_len,
            doc_freq,
        })
    }

    #[inline]
    pub fn get_postings_slice(&self, entry: &TermEntry) -> &[u8] {
        let start = self.postings_offset + entry.postings_offset as usize;
        let end = start + entry.postings_len as usize;
        &self.mmap[start..end]
    }

    pub fn get_term_str(&self, term_idx: usize) -> Option<&str> {
        if term_idx >= self.num_terms as usize {
            return None;
        }
        let off = self.terms_strings_offset + term_idx * 4;
        let s_start = u32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap()) as usize;
        let s_end = u32::from_le_bytes(self.mmap[off + 4..off + 8].try_into().unwrap()) as usize;
        let data_base = self.terms_strings_offset + (self.num_terms as usize + 1) * 4;
        let start = data_base + s_start;
        let end = data_base + s_end;
        if end > self.mmap.len() {
            return None;
        }
        std::str::from_utf8(&self.mmap[start..end]).ok()
    }
}

// ============================================================================
// FST Term Dictionary (using fst::Map)
// ============================================================================

pub struct TermDictionary {
    fst_map: Map<memmap2::Mmap>,
}

impl TermDictionary {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        let fst_map = Map::new(mmap)?;
        Ok(Self { fst_map })
    }

    pub fn build_from_index<P: AsRef<Path>>(
        fst_path: P,
        index: &MmapIndex,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let file = File::create(fst_path.as_ref())?;
        let writer = BufWriter::new(file);
        let mut builder = MapBuilder::new(writer)?;
        for idx in 0..index.num_terms as usize {
            if let Some(term) = index.get_term_str(idx) {
                builder.insert(term.as_bytes(), idx as u64)?;
            }
        }
        builder.finish()?;
        Self::open(fst_path)
    }

    #[inline]
    pub fn get(&self, term: &str) -> Option<u64> {
        self.fst_map.get(term.as_bytes())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.fst_map.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.fst_map.is_empty()
    }

    pub fn suggest_prefix(&self, prefix: &str, limit: usize) -> Vec<String> {
        let prefix_lower = prefix.to_lowercase();
        let prefix_matcher = Str::new(&prefix_lower).starts_with();
        let mut stream = self.fst_map.search(prefix_matcher).into_stream();

        let mut results = Vec::with_capacity(limit);
        while let Some((term_bytes, _val)) = stream.next() {
            if let Ok(term) = std::str::from_utf8(term_bytes) {
                results.push(term.to_string());
                if results.len() >= limit {
                    break;
                }
            }
        }
        results
    }

    pub fn fuzzy_terms(&self, term: &str, max_distance: u32, limit: usize) -> Vec<String> {
        let term_lower = term.to_lowercase();
        let dfa = match Levenshtein::new(&term_lower, max_distance) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        let mut stream = self.fst_map.search(&dfa).into_stream();
        let mut matched = Vec::with_capacity(limit);

        while let Some((bytes, _val)) = stream.next() {
            if let Ok(s) = std::str::from_utf8(bytes)
                && s != term_lower
            {
                matched.push(s.to_string());
                if matched.len() >= limit {
                    break;
                }
            }
        }
        matched
    }
}

// ============================================================================
// Tokenizer Pipeline
// ============================================================================

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

    pub fn tokenize(&self, text: &str) -> Vec<String> {
        let cleaned = self.regex.replace_all(text, " ").to_lowercase();
        cleaned
            .split_whitespace()
            .filter(|token| token.len() > 1 && !self.stop_words.contains(token))
            .map(|token| self.stemmer.stem(token).to_string())
            .collect()
    }
}

// ============================================================================
// Dynamic Snippet Extractor (Aho-Corasick on zero-copy memory-mapped slice)
// ============================================================================

fn generate_dynamic_snippet(content: &str, query_terms: &[String], target_len: usize) -> String {
    if content.is_empty() {
        return String::new();
    }

    let patterns: Vec<&str> = query_terms
        .iter()
        .map(|s| s.as_str())
        .filter(|s| s.len() > 1)
        .collect();
    if patterns.is_empty() {
        let snippet: String = content.chars().take(target_len).collect();
        return if content.len() > target_len {
            format!("{snippet}...")
        } else {
            snippet
        };
    }

    let ac = match AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build(&patterns)
    {
        Ok(aut) => aut,
        Err(_) => {
            let snippet: String = content.chars().take(target_len).collect();
            return format!("{snippet}...");
        }
    };

    let matches: Vec<(usize, usize)> = ac
        .find_iter(content)
        .map(|m| (m.start(), m.end()))
        .collect();

    if matches.is_empty() {
        let snippet: String = content.chars().take(target_len).collect();
        return if content.len() > target_len {
            format!("{snippet}...")
        } else {
            snippet
        };
    }

    // Locate the start of the window containing the densest cluster of matched tokens
    let mut best_start = matches[0].0;
    let mut max_density = 0;

    for (i, m) in matches.iter().enumerate() {
        let window_end = m.0 + target_len;
        let hits_in_window = matches[i..]
            .iter()
            .take_while(|next_m| next_m.0 < window_end)
            .count();

        if hits_in_window > max_density {
            max_density = hits_in_window;
            best_start = m.0;
        }
    }

    // Expand backwards slightly to avoid cutting off mid-word (safeguarding UTF-8 boundaries)
    let raw_start = best_start.saturating_sub(40);
    let start_offset = content.floor_char_boundary(raw_start);
    let safe_start = if start_offset > 0 {
        content[start_offset..best_start]
            .find(' ')
            .map(|off| start_offset + off + 1)
            .unwrap_or(start_offset)
    } else {
        0
    };

    let raw_end = (safe_start + target_len).min(content.len());
    let rough_end = content.ceil_char_boundary(raw_end);
    let safe_end = if rough_end < content.len() {
        content[rough_end..]
            .find(' ')
            .map(|off| rough_end + off)
            .unwrap_or(rough_end)
    } else {
        content.len()
    };
    let safe_end = safe_end.max(safe_start);

    let prefix = if safe_start > 0 { "..." } else { "" };
    let suffix = if safe_end < content.len() { "..." } else { "" };

    format!(
        "{}{}{}",
        prefix,
        content[safe_start..safe_end].trim(),
        suffix
    )
}

// ============================================================================
// Application State & DTOs
// ============================================================================

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

// ============================================================================
// Main Server Entrypoint
// ============================================================================

fn resolve_path(env_var: &str, candidates: &[&str]) -> String {
    if let Ok(val) = std::env::var(env_var) {
        return val;
    }
    for candidate in candidates {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    candidates[0].to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let index_path = resolve_path(
        "INDEX_PATH",
        &[
            "../indexer/index.bin",
            "indexer/index.bin",
            "index.bin",
            "/app/index.bin",
        ],
    );
    let docs_path = resolve_path(
        "DOCS_PATH",
        &[
            "../crawler/documents.bin",
            "crawler/documents.bin",
            "documents.bin",
            "/app/documents.bin",
        ],
    );
    let fst_path = resolve_path(
        "FST_PATH",
        &[
            "../indexer/dictionary.fst",
            "indexer/dictionary.fst",
            "dictionary.fst",
            "/app/dictionary.fst",
        ],
    );

    println!("Memory-mapping Inverted Index from {}...", index_path);
    let start_index = Instant::now();
    let index = Arc::new(MmapIndex::open(&index_path)?);
    println!(
        "Index mapped in {:?}: {} docs, {} terms, avg_len={:.2}",
        start_index.elapsed(),
        index.total_docs,
        index.num_terms,
        index.avg_doc_length
    );

    println!("Memory-mapping Document Store from {}...", docs_path);
    let start_docs = Instant::now();
    let doc_store = Arc::new(MmapDocStore::open(&docs_path)?);
    println!(
        "Document store mapped in {:?}: {} docs ready",
        start_docs.elapsed(),
        doc_store.doc_count()
    );

    println!("Opening Term Dictionary FST from {}...", fst_path);
    let start_fst = Instant::now();
    let dictionary = if Path::new(&fst_path).exists() {
        TermDictionary::open(&fst_path)?
    } else {
        println!("FST file not found at {fst_path}. Compiling on the fly from index.bin...");
        TermDictionary::build_from_index(&fst_path, &index)?
    };
    println!(
        "Term Dictionary ready in {:?} ({} terms in lexicon)",
        start_fst.elapsed(),
        dictionary.len()
    );

    let pipeline = TokenizerPipeline::new();
    let state = Arc::new(AppState {
        index,
        doc_store,
        pipeline,
        dictionary: Arc::new(dictionary),
    });

    println!(
        "Engine ready! Serving {} documents across {} terms with 0 RAM deserialization overhead.",
        state.index.total_docs, state.index.num_terms
    );

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(ui_handler))
        .route("/health", get(health_handler))
        .route("/api/search", get(search_handler))
        .route("/api/suggest", get(suggest_handler))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("Search UI & API running at http://localhost:8080");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn ui_handler() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        total_documents: state.index.total_docs,
        vocabulary_size: state.index.num_terms as usize,
    })
}

async fn suggest_handler(
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

// ============================================================================
// PageRank & BM25 Scoring Formulas
// ============================================================================

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
                did_you_mean: None,
                results: Vec::new(),
            }),
        );
    }

    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(10).clamp(1, 100);

    let start_time = Instant::now();

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
                did_you_mean: None,
                results: Vec::new(),
            }),
        );
    }

    let k1 = 1.5;
    let b = 0.75;
    let n = state.index.total_docs as f64;
    let mut scores: HashMap<u32, f64> = HashMap::new();

    // Fast BM25 scoring directly from memory-mapped postings & doc lengths
    for term in &query_terms {
        if let Some(term_idx) = state.dictionary.get(term)
            && let Some(entry) = state.index.get_term_entry(term_idx as usize)
        {
            let n_q = entry.doc_freq as f64;
            let idf = bm25_idf(n, n_q);
            let slice = state.index.get_postings_slice(&entry);
            let iter = PostingsIterator::new(slice, entry.doc_freq as usize);

            for posting in iter {
                let doc_len = state.index.get_doc_length(posting.doc_id) as f64;
                let tf = posting.term_frequency as f64;
                let term_score =
                    idf * bm25_tf_weight(tf, doc_len, state.index.avg_doc_length, k1, b);

                *scores.entry(posting.doc_id).or_insert(0.0) += term_score;
            }
        }
    }

    let alpha = params.alpha.unwrap_or(DEFAULT_ALPHA).clamp(0.0, 1.0);
    let epsilon = DEFAULT_EPSILON;

    let mut ranked: Vec<(u32, f64)> = scores
        .into_iter()
        .map(|(doc_id, bm25)| {
            let pr = state.index.get_pagerank(doc_id);
            let score = final_score(bm25, pr, alpha, epsilon);
            (doc_id, score)
        })
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let total_hits = ranked.len();
    let total_pages = total_hits.div_ceil(limit);

    // Fuzzy matching fallback if zero hits were scored
    let did_you_mean = if total_hits == 0 {
        let mut suggestions = Vec::new();
        for term in &query_terms {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function for test encoding
    fn encode_vbyte(mut val: u32, buf: &mut Vec<u8>) {
        while val >= 0x80 {
            buf.push(((val & 0x7F) as u8) | 0x80);
            val >>= 7;
        }
        buf.push((val & 0x7F) as u8);
    }

    // ------------------------------------------------------------------------
    // VByte Decoding Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_decode_vbyte_values() {
        let test_values = [0u32, 1, 127, 128, 16383, 16384, 65535, 1_000_000, u32::MAX];
        let mut buf = Vec::new();
        for &val in &test_values {
            encode_vbyte(val, &mut buf);
        }

        let mut offset = 0;
        let mut decoded = Vec::new();
        while let Some(v) = decode_vbyte(&buf, &mut offset) {
            decoded.push(v);
        }

        assert_eq!(decoded, test_values);
        assert_eq!(offset, buf.len());
    }

    #[test]
    fn test_decode_vbyte_truncated_and_overflow() {
        let mut offset = 0;
        assert_eq!(decode_vbyte(&[], &mut offset), None);

        offset = 0;
        assert_eq!(decode_vbyte(&[0x80], &mut offset), None);

        offset = 0;
        let malformed = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80];
        assert_eq!(decode_vbyte(&malformed, &mut offset), None);
    }

    // ------------------------------------------------------------------------
    // Delta Decoding via PostingsIterator Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_postings_iterator_delta_decoding() {
        let mut buf = Vec::new();
        // doc 10, tf 2
        encode_vbyte(10, &mut buf);
        encode_vbyte(2, &mut buf);
        // doc 15 (delta 5), tf 1
        encode_vbyte(5, &mut buf);
        encode_vbyte(1, &mut buf);
        // doc 40 (delta 25), tf 3
        encode_vbyte(25, &mut buf);
        encode_vbyte(3, &mut buf);

        let iter = PostingsIterator::new(&buf, 3);
        let postings: Vec<Posting> = iter.collect();

        assert_eq!(
            postings,
            vec![
                Posting {
                    doc_id: 10,
                    term_frequency: 2
                },
                Posting {
                    doc_id: 15,
                    term_frequency: 1
                },
                Posting {
                    doc_id: 40,
                    term_frequency: 3
                },
            ]
        );
    }

    #[test]
    fn test_postings_iterator_first_doc_zero() {
        let mut buf = Vec::new();
        // doc 0, tf 1
        encode_vbyte(0, &mut buf);
        encode_vbyte(1, &mut buf);
        // doc 3 (delta 3), tf 2
        encode_vbyte(3, &mut buf);
        encode_vbyte(2, &mut buf);

        let iter = PostingsIterator::new(&buf, 2);
        let postings: Vec<Posting> = iter.collect();

        assert_eq!(
            postings,
            vec![
                Posting {
                    doc_id: 0,
                    term_frequency: 1
                },
                Posting {
                    doc_id: 3,
                    term_frequency: 2
                },
            ]
        );
    }

    #[test]
    fn test_postings_iterator_empty() {
        let iter = PostingsIterator::new(&[], 0);
        let results: Vec<Posting> = iter.collect();
        assert!(results.is_empty());
    }

    // ------------------------------------------------------------------------
    // BM25 Scoring Arithmetic Tests
    // ------------------------------------------------------------------------

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

    // ------------------------------------------------------------------------
    // FinalScore Combined Ranking Tests
    // ------------------------------------------------------------------------

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
}
