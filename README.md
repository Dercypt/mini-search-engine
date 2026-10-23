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
| `SURFACE_FST_PATH` | `../indexer/surface.fst`, `surface.fst`, `/app/surface.fst` | Surface vocabulary FST path |

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
