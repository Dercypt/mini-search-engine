# Mini Search Engine

A full-text search engine in Rust with asynchronous web crawling, memory-mapped binary inverted indexing, and ranked retrieval.

## Tech Stack

- Rust
- Tokio
- Axum
- Reqwest
- Scraper
- Memmap2
- FST

## Features

- Asynchronous web crawling
- Binary inverted indexing
- BM25 and PageRank ranking
- WAND dynamic pruning
- Prefix suggestions and phrase matching

Installation
Bash

cargo build --release

Usage
Bash

./run_pipeline.sh

License

MIT
