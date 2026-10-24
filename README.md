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

## Installation

Clone the repository and build the project with Cargo:

```bash
git clone https://github.com/Dercypt/mini-search-engine.git
cd mini-search-engine
cargo build --release
```

## Usage

Run the end-to-end crawler, indexing, and search server pipeline:

```bash
./run_pipeline.sh
```

## License

MIT
