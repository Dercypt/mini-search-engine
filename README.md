# Mini Search Engine

A full-text search engine in Rust with asynchronous web crawling, memory-mapped binary inverted indexing, and ranked retrieval.

## Tech Stack

- Rust
- Tokio
- Axum
- Reqwest
- Scraper
- memmap2
- fst

## Features

- Asynchronous streaming web crawler with robots.txt compliance and URL deduplication.
- Binary inverted index with Variable Byte delta compression and memory-mapped reads.
- Relevance ranking combining BM25, PageRank, and WAND dynamic pruning.
- Quoted exact phrase matching and FST prefix completions.

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
