# ⚡ Mini Search Engine

A high-performance, modular search engine pipeline built entirely in **Rust**. Scrapes thousands of Wikipedia articles concurrently, builds an in-memory inverted index with **Roaring Bitmaps**, ranks documents using **Okapi BM25**, and serves search queries with sub-millisecond latency via an **Axum REST API** and embedded web interface.

---

## 🏗️ Architecture Overview

[ Wikipedia Seed ]
        │
        ▼
[ 1. Concurrent Crawler (/crawler) ]
  • Tokio Async Tasks & Semaphore Concurrency
  • FastBloom Filter Deduplication (0.1% FP)
  • Output: documents.json
        │
        ▼
[ 2. Inverted Indexer (/indexer) ]
  • Porter Stemmer & Stop-Word Filtering
  • Roaring Bitmaps for Fast Document Set Lookups
  • Precomputed Okapi BM25 IDF and Statistics
  • Output: index.json
        │
        ▼
[ 3. Search API & Web UI (/search_api) ]
  • Axum High-Performance HTTP Server
  • Dynamic Snippet Extraction & Query Term Highlighting
  • REST API: GET /api/search?q=...
  • Built-in Web UI: http://localhost:8080

---

## 🚀 Quickstart

### Option 1: Automated Shell Pipeline
Run the crawler, indexer, and search server sequentially with a single command:
./run_pipeline.sh

### Option 2: Docker Compose
docker compose up --build

Once started, navigate to **http://localhost:8080** in your browser.

---

## 📊 Performance Benchmarks

| Metric | Measured Value |
|---|---|
| **Crawl Throughput (1,000 Articles)** | ~40 seconds |
| **Inverted Index Construction** | ~230 ms |
| **Vocabulary Size** | 40,000+ unique stems |
| **Average Document Length** | ~1,790 terms/doc |
| **Average Query Latency** | **< 0.5 ms** |

---

## 🔌 API Reference

### `GET /api/search`
Query the search engine with BM25 ranking, dynamic snippets, and pagination.

#### Query Parameters
* `q` (string, required): The search terms.
* `page` (integer, optional): Page number (default: `1`).
* `limit` (integer, optional): Results per page (default: `10`, max: `100`).

#### Example Request
curl -s "http://localhost:8080/api/search?q=page+rank+algorithm&limit=1" | jq

#### Example Response
{
  "query": "page rank algorithm",
  "total_hits": 514,
  "page": 1,
  "limit": 1,
  "total_pages": 514,
  "execution_time_ms": 0.45,
  "results": [
    {
      "rank": 1,
      "doc_id": "b3ed93c031fd",
      "score": 9.4571,
      "title": "PageRank - Wikipedia",
      "url": "https://en.wikipedia.org/wiki/PageRank",
      "snippet": "PageRank ( PR ) is an algorithm used by Google Search to rank web pages in their search engine results. It is named after both the term \"web page\" and co-founder Larry Page . PageRank is..."
    }
  ]
}

### `GET /health`
Returns system health and index metrics.

#### Example Request
curl -s http://localhost:8080/health | jq

#### Example Response
{
  "status": "healthy",
  "total_documents": 1000,
  "vocabulary_size": 40980
}

---

## 🛠️ Tech Stack & Crates

* **Networking & Concurrency:** `tokio`, `reqwest`, `axum`, `tower-http`
* **Parsing & Scraping:** `scraper`, `regex`, `url`
* **Text Processing & IR:** `rust-stemmers`, `roaring` (Roaring Bitmaps), `fastbloom`
* **Serialization:** `serde`, `serde_json`, `bincode`, `sha2`, `hex`
