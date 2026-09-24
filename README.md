# Mini Search Engine
A lightweight, from-scratch search engine built in Rust. Features an asynchronous crawler, an in-memory inverted index, and sub-millisecond retrieval ranked via Okapi BM25.

[Live Demo](https://mini-search-engine-ijmt.onrender.com/)

---

## Features
* **Async Crawler:** Concurrent web scraper powered by `tokio` and `reqwest` with streaming binary disk output.
* **Memory-Mapped Inverted Index (mmap):** Zero-deserialization binary inverted index powered by `memmap2`.
* **VByte Compressed Binary Postings:** Delta-encoded postings lists compressed with Variable Byte (VByte) encoding.
* **FST Lexicon:** Fast finite-state transducer dictionary (`fst::Map`) for prefix suggestions and Levenshtein fuzzy search.
* **BM25 Ranking:** Relevance scoring ($k_1 = 1.5, b = 0.75$) evaluated directly from memory-mapped postings.
* **Zero RAM Choke:** Constant-memory streaming crawler and sub-microsecond random-access document reader scalable to 500,000+ pages.
* **Embedded UI:** Single-binary web interface and JSON API served via `axum`.
* **Fast Retrieval:** Sub-millisecond query latency directly against memory-mapped disk storage.

---

## Prerequisites
* Rust 1.85+ (`rustc --version` with Edition 2024 support)
* Docker & Docker Compose (optional, for Option 1)

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

`GET /api/search`

Search the in-memory index and return BM25-ranked results.

**Query Parameters**

| Param   | Type   | Required | Default | Description                  |
|---------|--------|----------|---------|-------------------------------|
| `q`     | string | yes      | —       | Search query                 |
| `page`  | int    | no       | `1`     | Page number                  |
| `limit` | int    | no       | `10`    | Results per page             |

**Example Request**
```bash
curl "http://localhost:8080/api/search?q=rust&page=1&limit=10"
```

**Example Response**
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
      "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
      "snippet": "A systems programming language focused on memory safety and performance..."
    }
  ]
}
```

**Response Fields**

| Field | Type | Description |
|---|---|---|
| `total_hits` | integer | Total matching documents in the index |
| `page` | integer | Current page number |
| `total_pages` | integer | Total available pages based on `limit` |
| `execution_time_ms` | string / float | Query execution time in milliseconds |
| `results` | array | List of matched document objects |
| `results[].rank` | integer | Position rank in query results |
| `results[].score` | string / float | BM25 relevance score (higher = more relevant) |
| `results[].title` | string | Page title |
| `results[].url` | string | Source Wikipedia URL |
| `results[].snippet` | string | Extracted body preview |

If `q` is missing, the API returns `400 Bad Request`.

---

## Architecture
```bash
crawler/      Scrapes Wikipedia articles -> documents.bin (mmap-ready binary store)
indexer/      Parses documents and builds -> index.bin (VByte postings) + dictionary.fst
search_api/   Zero-allocation mmap BM25 ranker + Axum web interface
```

---

## Indexing Architecture: Batch vs. Segment-Based (Lucene & Tantivy)

### Current Architecture: Offline Batch Processing
In this project, indexing is implemented as an **offline batch pipeline**:
1. **Raw Document Store:** The crawler sequentially downloads documents into `documents.bin`.
2. **Monolithic Index Build:** The indexer loads all documents into memory, computes a global PageRank pass, delta-encodes and VByte-compresses postings, and writes monolithic artifacts (`index.bin` and `dictionary.fst`).
3. **Static Mmap Serving:** The search API memory-maps these artifacts read-only for sub-millisecond BM25 querying.

**Limitations of Offline Batch Indexing:**
* **Stop-the-World Updates:** Inserting, modifying, or deleting a document requires re-processing the entire corpus ($O(N)$ re-indexing cost).
* **Stale Search Results:** Documents are not searchable until the entire batch indexing pipeline finishes.
* **In-Place Mutation Overhead:** Compressed, delta-encoded postings lists cannot be easily mutated in-place on disk without expensive offset shifting and fragmentation.

---

### Modern Engine Architecture (Lucene, Tantivy): Immutable Segments & Merges

Modern production search engines like **Apache Lucene** and **Tantivy** address incremental updates by adopting an **LSM-tree (Log-Structured Merge-Tree)** inspired segment architecture:

```
                  +----------------------------------+
                  |    Incoming Documents (CRUD)     |
                  +----------------------------------+
                                   |
                                   v
             +---------------------------------------------+
             | In-Memory Buffer (MemTable / SegmentWriter) |
             +---------------------------------------------+
                                   |
                     flush threshold (size / time)
                                   v
    +---------------------------------------------------------------+
    |                      Immutable Segments                       |
    |  +---------------+   +---------------+   +---------------+    |
    |  |   Segment 1   |   |   Segment 2   |   |   Segment 3   |    |
    |  | - Postings    |   | - Postings    |   | - Postings    |    |
    |  | - FST Lexicon |   | - FST Lexicon |   | - FST Lexicon |    |
    |  | - Del Bitset  |   | - Del Bitset  |   | - Del Bitset  |    |
    |  +---------------+   +---------------+   +---------------+    |
    +---------------------------------------------------------------+
                                   |
                        background merge policy
                                   v
    +---------------------------------------------------------------+
    |              Merged Segment (Purged Tombstones)               |
    |  +---------------------------------------------------------+  |
    |  |                       Segment 1+2                       |  |
    |  +---------------------------------------------------------+  |
    +---------------------------------------------------------------+
```

#### 1. In-Memory Buffers & Segment Flushing
* **RAM Buffers (`SegmentWriter`):** Incoming documents are indexed into an in-memory buffer (`DocumentsWriterPerThread` in Lucene, `SegmentWriter` in Tantivy) and appended to a Write-Ahead Log (WAL) for crash resilience.
* **Immutable Flush:** Once the memory buffer reaches a threshold (e.g. 128MB–512MB) or an explicit flush occurs, it is serialized to disk as an **immutable segment**. Each segment is a fully self-contained mini-index with its own postings, term dictionary (FST or block-indexed), and stored fields.
* **Concurrent Lock-Free Reads:** Because written segments are strictly immutable:
  - Search threads query segments concurrently via `mmap` without read-write locking contention.
  - OS page cache pages remain warm without invalidation storms.

#### 2. Tombstone-Based Updates and Deletions
* **Append-Only Semantics:** Rather than mutating postings in place, an update is executed as a logical `Delete(doc_id)` followed by an `Insert(new_doc)`.
* **Deletion Bitmaps (Tombstones):** Deleted document IDs are marked in a lightweight bitset (e.g., Roaring Bitmaps or Tantivy `.del` files).
* **Query-Time Masking:** Postings iterators check matches against the segment's deletion bitmap, transparently filtering out deleted documents during BM25 evaluation.

#### 3. Background Segments Merging (Compaction)
* **Merge Policies (Tiered / Log-Merge):** As incremental flushes create numerous small segments, search latency and file handle usage increase ($O(S)$ search cost across $S$ segments). A background merge policy (e.g., Lucene's `TieredMergePolicy` or Tantivy's `LogMergePolicy`) continually identifies segments of comparable size tiers.
* **Multi-Way Stream Merging:** Merging performs a k-way merge of sorted postings streams, reassigns internal document IDs, and physically purges tombstoned documents to reclaim storage.
* **Atomic Snapshot Swapping:** Once the merged segment is finalized, the index metadata is updated atomically via a commit point. Active queries continue reading older segments until their cursors finish, after which obsolete segment files are safely unlinked.

---

## License
[MIT](LICENSE)
