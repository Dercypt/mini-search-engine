# Mini Search Engine - System Context

## Overview
A custom, high-performance web search engine built from scratch in Rust (2024 Edition). The system crawls web documents, builds an in-memory inverted index with mathematical relevance scoring (Okapi BM25) and roaring bitmap-accelerated set operations, serves sub-millisecond queries via an asynchronous REST API, and delivers an embedded, responsive web UI.

---

## Phase Status Summary
- **Phase 1: Web Crawler (`/crawler`)** — **COMPLETE**
- **Phase 2: Inverted Indexer & Ranking Engine (`/indexer`)** — **COMPLETE**
- **Phase 3: Query Engine & REST API (`/search_api`)** — **COMPLETE**
- **Phase 4: Embedded UI, Containerization & Production Deployment** — **COMPLETE**
- **Phase 5: Algorithmic Search Enhancements (FST, Fuzzy & Dynamic Snippets)** — **IN PROGRESS**
- **Phase 6: Hybrid Retrieval (Dense Vector + BM25)** — **UPCOMING**
- **Phase 7: Benchmarking, CI/CD & Production Polish** — **UPCOMING**

---

## Phase 1 Architecture (`/crawler`)

### 1. Objective
Traverse web pages starting from a seed URL, parse structured metadata and full body text, capture outbound link graphs, filter administrative noise, and persist normalized document snapshots.

### 2. Tech Stack & Dependencies
- **Language:** Rust (2024 Edition)
- **Runtime:** `tokio` (Multi-threaded asynchronous task scheduling)
- **HTTP Client:** `reqwest` (Connection pooling, custom User-Agent headers)
- **HTML Parser:** `scraper` (Scoped selectors for body paragraphs `div#bodyContent p` and links `div#bodyContent a[href]`)
- **Deduplication:** `fastbloom` (Probabilistic Bloom filter with 10,000 capacity, 0.1% false-positive rate)
- **Serialization & Hashing:** `serde`, `serde_json`, `sha2`, `hex`, `url`, `regex`

### 3. Key Components & Implementation
* **URL Frontier:** `tokio::sync::mpsc::unbounded_channel` for asynchronous queueing of newly discovered links.
* **Politeness & Rate Limiting:** `tokio::sync::Semaphore` capped at 5 concurrent worker tasks with an 80ms sleep delay per request.
* **URL Normalization:** Standardized through `url::Url` by stripping query parameters and fragment anchors (`#section`).
* **Noise Filtering:** Regex filters discarding non-article Wikipedia namespaces (`/Special:`, `/Talk:`, `/User:`, `/File:`, `/Template:`, `?action=`).
* **Output Schema:** 1,000 crawled pages persisted to `documents.json`.

---

## Phase 2 Architecture (`/indexer`)

### 1. Objective
Parse raw document JSON corpora, run text preprocessing (tokenization, lowercasing, stop-word elimination, Porter stemming), construct an inverted index mapping stemmed terms to document IDs and term frequencies, score documents via Okapi BM25, and serialize the index to disk.

### 2. Tech Stack & Dependencies
- **Language:** Rust (2024 Edition)
- **Bitset Operations:** `roaring` (Roaring Bitmaps for compressed, high-speed document set operations)
- **Stemming:** `rust-stemmers` (Porter Stemming Algorithm for English)
- **Text Processing & Regex:** `regex` (Alpha-numeric token boundary extraction)
- **Serialization:** `serde`, `serde_json`, `bincode`

### 3. Key Components & Implementation
* **Tokenizer Pipeline:**
  * Regex-based non-alphanumeric character replacement and lowercasing.
  * Standard English stop-word filtering (~120 common stop words).
  * Algorithmic word stemming via `rust_stemmers::Stemmer`.
* **Field Weighting:** Document titles receive double term weighting ($2\times$) during posting list construction to prioritize exact title matches.
* **Roaring Bitmap Integration:** Each unique vocabulary term maintains an in-memory `RoaringBitmap` tracking document occurrences for instant multi-term set operations and filtering.
* **Okapi BM25 Relevance Scoring:**
  * $\text{IDF}(q) = \ln\left(1 + \frac{N - n_q + 0.5}{n_q + 0.5}\right)$
  * $\text{Score}(D, Q) = \sum_{q \in Q} \text{IDF}(q) \cdot \frac{f(q, D) \cdot (k_1 + 1)}{f(q, D) + k_1 \cdot \left(1 - b + b \cdot \frac{|D|}{\text{avgdl}}\right)}$
  * Parameters calibrated to $k_1 = 1.2$, $b = 0.75$.
* **Storage & Index Structure:**
  * Inverted index mapping `term -> Vec<Posting { doc_id, term_frequency }>`.
  * Document metadata map storing internal integer IDs, hex hashes, URLs, titles, and token counts.
  * Serialized on-disk index stored as `indexer/index.json`.

### 4. Corpus & Index Metrics
- **Indexed Documents:** 1,000 documents
- **Ingestion & Tokenization Time:** ~232 ms
- **Unique Vocabulary Stems:** ~39,755 to 42,126 terms
- **Index Serialization Time:** ~206 ms
- **Query Evaluation Latency:** $<1\text{ ms}$ per multi-term query

---

## Phase 3 Architecture (`/search_api`)

### 1. Objective
Expose high-performance, asynchronous REST endpoints that accept user queries, parse search terms through an identical tokenization/stemming pipeline, score candidates against the in-memory inverted index using Okapi BM25, extract contextual preview snippets, and return paginated JSON search results with sub-millisecond execution times.

### 2. Tech Stack & Dependencies
- **Language:** Rust (2024 Edition)
- **Web Framework:** `axum` (v0.7/v0.8 with ergonomic routing and extractor state management)
- **Runtime:** `tokio` (Multi-threaded async executor)
- **Middleware:** `tower-http` (CORS handling with wildcard access, request tracing)
- **Logging & Tracing:** `tracing`, `tracing-subscriber`
- **Text & Serialization:** `serde`, `serde_json`, `regex`, `rust-stemmers`

### 3. Key Components & Implementation
* **Shared In-Memory State (`AppState`):** 
  * Inverted index (`index.json`) and raw document store (`documents.json`) loaded into memory at server startup wrapped in `Arc<AppState>`.
  * Eliminates runtime disk I/O bottlenecks during search operations.
* **Synchronized Tokenization Pipeline:**
  * Mirrors the indexer’s text normalization, stop-word removal, and Porter stemmer to ensure precise term matches against the posting lists.
* **BM25 Scoring & Ranking Engine:**
  * Computes dynamic term weights and document lengths per query.
  * Aggregates multi-term postings and sorts results descending by BM25 relevance score.
* **Pagination & Query Parameters:**
  * Deserializes `q` (required string), `page` (optional uint, default: 1), and `limit` (optional uint, default: 10).
  * Emits query metadata including `total_hits`, `total_pages`, current `page`, and `execution_time_ms`.
* **Endpoints:**
  * `GET /health` — Verifies server health, loaded document count, and vocabulary size.
  * `GET /api/search?q=<query>&page=<int>&limit=<int>` — Primary search endpoint returning scored hits and snippets.

---

## Phase 4 Architecture (UI, Dockerization & Production Deployment)

### 1. Objective
Build an embedded, zero-dependency responsive frontend interface directly inside the single binary, package the entire build and pipeline into a reproducible multi-stage Docker environment, deploy the live service to Render, and write minimalist, production-ready documentation.

### 2. Tech Stack & Dependencies
- **Frontend:** Vanilla HTML5, Modern CSS, Vanilla ES6+ JavaScript (zero external framework overhead)
- **Typography:** IBM Plex Sans, IBM Plex Mono, Fraunces (Google Fonts)
- **Containerization:** Multi-stage `Dockerfile` (`rust:latest` builder, Debian bookworm runtime), `docker-compose.yml`
- **Deployment Platform:** Render Web Services (Docker runtime, auto-deployed via GitHub)
- **Automation:** `run_pipeline.sh`

### 3. Key Components & Implementation
* **Zero-Dependency Embedded UI (`search_api/src/index.html`):**
  * Embedded and compiled directly into the binary via Axum static route serving (`GET /`).
  * Features FLIP layout transitions moving the search bar from the hero center to the sticky navigation rail.
  * Client-side term highlighting using `<mark class="hit">` and simulated offline fallback data.
  * Keyboard navigation shortcuts: `/` to focus search input, `Esc` to reset to hero view.
* **Mobile & Tablet Responsiveness:**
  * Stacked card layouts on narrow viewports ($< 680\text{px}$) converting horizontal metadata tabs into compact top bars.
  * Responsive sticky header rail hiding secondary text to prevent layout breaks on small screens.
  * Standard $16\text{px}$ input font sizing to prevent iOS Safari auto-zoom on focus.
* **Docker Multi-Stage Build:**
  * Builder stage compiles `crawler`, `indexer`, and `search_api` while executing the crawling and indexing steps during image compilation.
  * Runtime stage packages only the compiled `search_api` binary and generated `index.json`/`documents.json` into a minimal container.
* **Production Documentation & Quality Assurance:**
  * Minimalist `README.md` containing dual quickstart tracks (Docker vs. Local Cargo), complete API parameter tables, and JSON response schemas.
  * Resolved runtime port binding conflicts (`AddrInUse` on port `8080`).

---

## Phase 5 Architecture: Algorithmic Search Enhancements — *CURRENT*

### 1. Objective
Enhance search ergonomics and recall by implementing zero-overhead prefix autocomplete, microsecond Levenshtein typo correction, and single-pass multi-pattern dynamic snippet extraction.

### 2. Tech Stack & Dependencies
- **Finite State Transducers (FST):** `fst` (Immutable, memory-mapped prefix automatons for $O(\text{prefix len})$ lookups)
- **Fuzzy Levenshtein DFA:** `fst-levenshtein` (Microsecond edit-distance state machine intersection for $d \le 2$)
- **Multi-Pattern Matching:** `aho-corasick` (Single-pass $O(n)$ token cluster locator for dynamic window generation)

### 3. Key Components & Target Implementation
* **Prefix Autocomplete (`GET /api/suggest?q=<prefix>&limit=5`):**
  * Compile sorted vocabulary stems and raw dictionary tokens into an immutable `fst::Set`.
  * Traverse prefix automatons via `fst::automaton::Str` to suggest completions in real time.
* **Fuzzy Typo Tolerance:**
  * Compile query terms into Levenshtein DFAs and intersect against the term FST.
  * If a query returns 0 hits, automatically generate *"Did you mean...?"* candidate corrections.
* **Dynamic Aho-Corasick Snippet Windowing:**
  * Scan raw document text in a single pass to find the densest cluster of matched query tokens.
  * Extract a dynamic ~160-character snippet snapped cleanly to word boundaries, replacing static prefix truncation.

---

## Upcoming Roadmap (Phase 6+)

### Phase 6: Hybrid Retrieval (Dense Vector + BM25)
* Local embeddings generation using `fastembed-rs` (`all-MiniLM-L6-v2`) without Python runtime dependencies.
* In-memory cosine similarity search merged with BM25 using Reciprocal Rank Fusion (RRF).

### Phase 7: Benchmarking, CI/CD & Visual Polish
* Automated load testing using `k6` to record and publish $p50, p95, p99$ latency profiles.
* GitHub Actions CI pipeline running `cargo test`, `cargo fmt`, `cargo clippy`, and Docker verification.
* Visual demo assets (optimized GIFs) embedded directly in repository documentation.
