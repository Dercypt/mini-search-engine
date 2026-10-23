# Mini Search Engine

A lightweight full-text search engine built from scratch in Rust. It crawls web pages, builds a zero-copy memory-mapped binary inverted index, and serves ranked search results using BM25, PageRank, and WAND dynamic pruning.

[Live Demo](https://mini-search-engine-ijmt.onrender.com/)

---

## How It Works

1. Crawler (`crawler`): Asynchronously fetches web pages and stores raw content in a compact binary document format.
2. Indexer (`indexer`): Tokenizes content, computes PageRank, and serializes a compressed binary inverted index and prefix dictionary.
3. Search API & Web UI (`search_api`): Uses memory-mapped files to serve ranked search queries and prefix suggestions with low latency and minimal memory overhead.

---

## Quickstart

### Native

Prerequisites: Rust 1.85+

```bash
git clone https://github.com/Dercypt/mini-search-engine.git
cd mini-search-engine
./run_pipeline.sh
```

Open `http://localhost:8080` in your browser.

### Docker

```bash
docker compose up --build
```

---

## API Endpoints

- `GET /api/search?q=<query>&page=1&limit=10`: Evaluates BM25 and PageRank retrieval with WAND dynamic pruning. Supports quoted exact phrase queries.
- `GET /api/suggest?q=<prefix>&limit=5`: Returns prefix completions from the dictionary.
- `GET /health`: Returns service health and index statistics.

---

## Verification

Run code formatting, linter, and test suites:

```bash
./verify.sh
```

---

## License

[MIT](LICENSE)
