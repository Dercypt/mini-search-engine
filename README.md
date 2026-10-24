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

## Installation

Clone the repository and build the workspace using Cargo:

```bash
git clone https://github.com/Dercypt/mini-search-engine.git
cd mini-search-engine
cargo build --release
```

## Usage

Run the complete pipeline to crawl, index, and start the search service:

```bash
./run_pipeline.sh
```

Alternatively, run the service with Docker Compose:

```bash
docker compose up --build
```

## License

This project is licensed under the MIT License.
