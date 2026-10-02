# System Laws & Invariants

This document establishes the foundational system invariants for the `mini-search-engine` workspace. These laws are non-negotiable and govern all development, refactoring, and maintenance.

## Primary Domain
High-performance, zero-deserialization binary inverted index and information retrieval search engine in Rust. The system comprises an asynchronous streaming crawler (`crawler`), a binary indexer and lexicon generator (`indexer`), and an in-memory/memory-mapped retrieval engine with BM25 ranking and Axum HTTP serving (`search_api`).

---

## Non-Negotiable System Invariants

### Law 1: Binary Storage Protocol & Magic Header Invariant
- **Specification**:
  - The document store (`documents.bin`) must strictly adhere to the `MSEDOC01` protocol: a fixed 64-byte header (`magic[8]`, `doc_count[4]`, `index_offset[8]`, 44-byte reserved padding) followed by sequential document payloads and a trailing doc-offset table.
  - The inverted index (`index.bin`) must strictly adhere to the `MSEIDX01` protocol: a fixed 128-byte header containing total doc count, average doc length, and explicit 64-bit byte offsets pointing to doc lengths, metadata tables, terms table, and postings blocks.
- **Guarantee**: Memory-mapped readers (`memmap2`) must never read out-of-bounds offsets, produce misaligned byte slices, or trigger segfaults. All internal file references must be validated against actual buffer boundaries.

### Law 2: Posting List Delta-Encoding Monotonicity Invariant
- **Specification**:
  - Within every term postings list, document IDs (`doc_id`) must be strictly monotonically increasing:
    $$\text{doc\_id}_i > \text{doc\_id}_{i-1}, \quad \forall i > 0$$
  - Within any single document's posting record, term positions must be strictly monotonically increasing:
    $$\text{pos}_j > \text{pos}_{j-1}, \quad \forall j > 0$$
- **Guarantee**: Variable Byte (VByte) compression relies on positive deltas ($\Delta \text{doc\_id} - 1 \ge 0$, $\Delta \text{pos} - 1 \ge 0$). Duplicate or descending doc IDs or positions will corrupt the delta decompression stream and invalidate WAND cursor traversal.

### Law 3: Relevance Scoring Stability, Finiteness, and Boundedness
- **Specification**:
  - All BM25 relevance scores, WAND term upper bounds ($U_t$), and PageRank scores must be strictly finite, non-negative floating-point values ($0.0 \le \text{score} < \infty$). Under no circumstances may scores evaluate to `NaN` or `±Infinity`.
  - BM25 algorithmic parameters must satisfy standard domains: $k_1 \ge 0$, $0.0 \le b \le 1.0$, and average document length $\text{avg\_len} > 0.0$.
  - In WAND dynamic pruning, each term upper bound $U_t$ must be a true upper bound: for all documents $d$, $\text{score}(d, t) \le U_t$.

### Law 4: API Contract Reliability & Graceful Degradation
- **Specification**:
  - Public HTTP endpoints (`GET /api/search`, `GET /api/suggest`, `GET /health`) must adhere strictly to JSON contracts.
  - Queries with missing or whitespace-only parameter `q` must immediately return `400 Bad Request`.
  - Queries with terms not present in the lexicon, zero matching postings, or out-of-range pagination must return HTTP 200 with a valid empty results payload (`"results": []`, `"total_hits": 0`), never causing panics, 500 crashes, or unbounded memory allocations.

---

## Governance & Immutability Rule

> [!IMPORTANT]
> **Strict Agent Immutability Mandate**:
> All files located in `tests/laws/` and `LAWS.md` are **strictly read-only and immutable** to all autonomous agents.
> Agents are expressly forbidden from modifying, moving, overriding, or deleting law definitions or law verification tests to bypass failing tests. If an implementation cannot satisfy `LAWS.md`, the code must be revised to conform, or the agent must escalate to the user.
