# Mini Search Engine

A lightweight, from-scratch search engine built in Rust. Features an asynchronous crawler, an in-memory inverted index, and sub-millisecond retrieval ranked via Okapi BM25.

[Live Demo](https://mini-search-engine-ijmt.onrender.com/)

---

## Features

* **Async Crawler:** Concurrent web scraper powered by `tokio` and `reqwest`.
* **Inverted Index:** In-memory postings list with Porter stemming and token normalization.
* **BM25 Ranking:** Relevance scoring ($k_1 = 1.2, b = 0.75$) matching corpus rarity.
* **Embedded UI:** Single-binary web interface and JSON API served via `axum`.
* **Sub-Millisecond Speed:** Sub-millisecond query latency directly against memory.

---

## Quickstart

### Option 1: With Docker

```bash
git clone https://github.com/Dercypt/mini-search-engine.git
cd mini-search-engine
docker compose up --build
```
Open `http://localhost:8080`.

### Option 2: Manual Build

```bash
# 1. Scrape Wikipedia
cd crawler && cargo run --release && cd ..

# 2. Build inverted index
cd indexer && cargo run --release && cd ..

# 3. Serve API & Web UI
cd search_api && cargo run --release
```

---

## API

HTTP: `GET /api/search?q=rust&page=1&limit=10`

JSON:
```json
{
  "total_hits": 18,
  "page": 1,
  "total_pages": 2,
  "execution_time_ms": "0.38",
  "results": [
    {
      "rank": 1,
      "score": "6.4210",
      "title": "Rust (programming language)",
      "url": "[https://en.wikipedia.org/wiki/Rust_(programming_language](https://en.wikipedia.org/wiki/Rust_(programming_language))",
      "snippet": "A systems programming language focused on memory safety and performance..."
    }
  ]
}
```

---

## Architecture

```bash
crawler/      Scrapes Wikipedia articles -> documents.json
indexer/      Parses documents and builds -> index.json
search_api/   In-memory BM25 ranker + Axum web interface
```

---

## License

[MIT](LICENSE)
