# Mini Search Engine

A full-text search engine in Rust with asynchronous web crawling, memory-mapped binary inverted indexing, and ranked retrieval.

## Tech Stack

- Rust
- Tokio
- Axum
- Reqwest & Scraper
- Memmap2
- FST

## Features

- Asynchronous web crawling with robots.txt compliance and Bloom filter deduplication.
- Zero-copy binary inverted indexing using variable-byte delta encoding.
- Ranked search combining BM25 scoring, PageRank, and WAND dynamic pruning.
- Quoted exact phrase matching and FST-based prefix suggestions.
