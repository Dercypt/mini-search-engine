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

Build the release binaries using Cargo:

```bash
cargo build --release
```

## Usage

Run the complete pipeline to crawl, index, and start the search service:

```bash
./run_pipeline.sh
```

## License

MIT License. See [LICENSE](LICENSE) for details.
