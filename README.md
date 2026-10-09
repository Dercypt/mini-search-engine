# Mini Search Engine

A fast, lightweight, from-scratch search engine built in Rust. Features an asynchronous web crawler, a zero-deserialization memory-mapped binary inverted index, and sub-millisecond full-text retrieval ranked with Okapi BM25, PageRank, and WAND dynamic pruning.

[Live Demo](https://mini-search-engine-ijmt.onrender.com/)

---

## Key Highlights

- **Zero-Deserialization Engine (`memmap2`)**: Postings and documents are queried directly from disk-backed memory maps without RAM deserialization overhead.
- **VByte-Compressed Postings**: Postings lists are delta-encoded and compressed using Variable Byte (VByte) encoding for compact disk footprint and fast scanning.
- **FST Lexicon**: Finite-state transducer dictionary (`fst::Map`) enabling sub-millisecond term lookups, prefix auto-suggestions, and typo-tolerant search.
- **Hybrid Scoring**: Combines Okapi BM25 ($k_1 = 1.5, b = 0.75$) relevance with link-graph PageRank scores.
- **WAND Dynamic Pruning**: Weak AND (WAND) top-$k$ candidate pruning algorithm avoids scoring non-competitive documents.
- **Exact Phrase Matching**: Positional postings enable exact quoted phrase filtering (e.g. `"search engine"`).
- **Asynchronous Crawler**: Concurrent Wikipedia crawler built with `tokio`, `reqwest`, robots.txt compliance, and bloom-filter deduplication.
- **Embedded Web UI & REST API**: Single-binary web service with responsive search interface and JSON API served via `axum`.

---

## Architecture & Data Flow

The project is split into three modular crates forming an offline-to-online retrieval pipeline:

```
[ Web / Wikipedia ]
        │
        ▼ (Asynchronous Web Crawler)
   ┌─────────┐
   │ crawler │  ──> documents.bin (MSEDOC01 binary document store)
   └─────────┘
        │
        ▼ (Batch Indexer & Lexicon Builder)
   ┌─────────┐  ──> index.bin (MSEIDX01 VByte inverted index + metadata)
   │ indexer │  ──> dictionary.fst (FST term dictionary)
   └─────────┘
        │
        ▼ (Memory-Mapped Retrieval Engine)
 ┌──────────────┐
 │  search_api  │  <── Serves Web UI & REST API on port 8080 (0 RAM choke)
 └──────────────┘
```

### Components

| Crate | Responsibility | Key Output / Role |
|-------|----------------|-------------------|
| [`crawler`](crawler/) | Asynchronously scrapes web pages with robots.txt parsing, HTML extraction, bloom-filter deduplication, and link graph collection. | `documents.bin` |
| [`indexer`](indexer/) | Tokenizes raw documents, computes global PageRank via power iteration, delta-encodes positional postings, and serializes binary index files. | `index.bin`, `dictionary.fst` |
| [`search_api`](search_api/) | Mmaps index files read-only, evaluates BM25/WAND queries, runs prefix auto-suggest, generates dynamic snippets, and hosts the web UI. | HTTP server (`:8080`) |

### Binary Storage Formats

- **`documents.bin` (`MSEDOC01`)**: 64-byte magic header, sequential document records (URL, title, body, links), and a trailing document offset table for $O(1)$ random lookups.
- **`index.bin` (`MSEIDX01`)**: 128-byte magic header containing doc count, average document length, and 64-bit byte offsets pointing to doc lengths, PageRank scores, term metadata tables, and delta-compressed VByte posting blocks.
- **`dictionary.fst`**: Monotonically sorted finite-state transducer mapping term strings directly to metadata byte offsets in `index.bin`.

> **Architecture Note**: This engine uses an offline batch indexing model designed for maximum sequential compression and zero-copy mmap reads. Real-time incremental search engines typically expand on this using LSM segment flushes and background compaction merges (e.g., Lucene or Tantivy).

---

## Prerequisites

- **Rust 1.85+** (Edition 2024 support)
- **Docker & Docker Compose** (optional)

---

## Quickstart

### Option 1: One-Command Pipeline (Recommended)

Run the end-to-end crawler, indexer, and search server with the provided script:

```bash
git clone https://github.com/Dercypt/mini-search-engine.git
cd mini-search-engine
./run_pipeline.sh
```

Open `http://localhost:8080` in your browser.

---

### Option 2: Step-by-Step Manual Execution

```bash
# 1. Crawl pages (defaults to 1000 Wikipedia pages)
cd crawler
cargo run --release -- --max-pages 500 --seed "https://en.wikipedia.org/wiki/Search_engine"
cd ..

# 2. Build inverted index and FST dictionary
cd indexer
cargo run --release
cd ..

# 3. Start Search API & Web UI
cd search_api
cargo run --release
```

---

### Option 3: Docker

```bash
docker compose up --build
```

Access the UI at `http://localhost:8080`.

---

## API Reference

The `search_api` server provides the following HTTP endpoints:

### 1. Search Query

`GET /api/search`

Executes BM25 and PageRank retrieval with WAND dynamic pruning. Supports standard keyword queries and quoted exact-match phrases (e.g. `"search engine"`).

**Query Parameters**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `q` | string | Yes | — | Search query (returns `400 Bad Request` if empty) |
| `page` | integer | No | `1` | Page number for pagination |
| `limit` | integer | No | `10` | Results per page (clamped 1–100) |
| `alpha` | float | No | `0.85` | Score blend between BM25 (`1.0`) and PageRank (`0.0`) |

**Example Request**
```bash
curl "http://localhost:8080/api/search?q=rust+programming&page=1&limit=5"
```

**Example Response**
```json
{
  "query": "rust programming",
  "total_hits": 24,
  "page": 1,
  "limit": 5,
  "total_pages": 5,
  "execution_time_ms": 0.38,
  "results": [
    {
      "rank": 1,
      "doc_id": "12",
      "score": 6.421,
      "pagerank": 0.00185,
      "title": "Rust (programming language)",
      "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
      "snippet": "A systems programming language focused on memory safety and performance..."
    }
  ]
}
```

---

### 2. Auto-Suggest / Prefix Lookup

`GET /api/suggest`

Returns term completions from the FST dictionary matching a prefix.

**Query Parameters**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `q` | string | Yes | — | Prefix string to match |
| `limit` | integer | No | `5` | Maximum suggestions (clamped 1–20) |

**Example Request**
```bash
curl "http://localhost:8080/api/suggest?q=algo&limit=3"
```

**Example Response**
```json
[
  "algorithm",
  "algorithmic",
  "algorithms"
]
```

---

### 3. Service Health

`GET /health`

Returns service health, indexed document count, and vocabulary size.

**Example Request**
```bash
curl "http://localhost:8080/health"
```

**Example Response**
```json
{
  "status": "healthy",
  "total_documents": 500,
  "vocabulary_size": 18450
}
```

---

## Configuration & CLI Options

### Crawler (`crawler`)

```bash
cargo run --release -- [OPTIONS]
```

| Flag | Option | Default | Description |
|------|--------|---------|-------------|
| `-m` | `--max-pages` | `1000` | Target page limit |
| `-s` | `--seed` | Wikipedia Search engine | Starting seed URL |
| `-o` | `--output` | `documents.json` | Base path for `.bin` and `.json` outputs |

### Search API (`search_api`)

Configurable via environment variables:

| Variable | Default Fallback Paths | Description |
|----------|------------------------|-------------|
| `INDEX_PATH` | `../indexer/index.bin`, `index.bin`, `/app/index.bin` | Inverted index path |
| `DOCS_PATH` | `../crawler/documents.bin`, `documents.bin`, `/app/documents.bin` | Document store path |
| `FST_PATH` | `../indexer/dictionary.fst`, `dictionary.fst`, `/app/dictionary.fst` | FST dictionary path |

---

## Verification & Testing

Verify code style, lints, and test suites across all crates in one step:

```bash
./verify.sh
```

The verification pipeline executes:
1. **Formatting**: `cargo fmt --check` across `crawler`, `indexer`, and `search_api`.
2. **Static Analysis**: `cargo clippy -- -D warnings` with zero allowed warnings.
3. **Unit & Integration Tests**: `cargo test` across all crates.
4. **Law Verification**: Invariant tests validating system laws defined in [`LAWS.md`](LAWS.md).

---

## Governance & System Invariants

This repository enforces strict architectural guarantees:
- **[`LAWS.md`](LAWS.md)**: Non-negotiable system invariants (binary headers `MSEDOC01`/`MSEIDX01`, monotonic delta-encoding, bounded finite BM25 scores, API JSON contracts).
- **[`PRINCIPLES.md`](PRINCIPLES.md)**: Engineering defaults (strongly-typed errors, zero untyped bypasses, strict safety, escalation triggers).
- **[`HARNESS.md`](HARNESS.md)**: Quality gates and verification lifecycle rules.
- **[`AGENTS.md`](AGENTS.md)**: Operating instructions and autonomous verification protocols for AI coding assistants.

---

## License

[MIT](LICENSE)
