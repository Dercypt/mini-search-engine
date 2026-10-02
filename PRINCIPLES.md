# Engineering Principles & Operational Defaults

This document governs the coding standards, default behaviors, and escalation triggers across the `mini-search-engine` repository.

---

## Escalation Triggers

Autonomous agents and engineers must **halt and explicitly prompt the user for confirmation** before proceeding if any proposed change encounters any of the following triggers:

1. **Adding External Dependencies**:
   - Introducing new third-party crates or upgrading existing dependencies in `crawler/Cargo.toml`, `indexer/Cargo.toml`, or `search_api/Cargo.toml`.
2. **Modifying Public API Routes or Contracts**:
   - Adding, altering, or removing HTTP routes (`/api/search`, `/api/suggest`, `/health`, `/`).
   - Changing query parameter contracts (e.g. `q`, `page`, `limit`), HTTP response status codes, or top-level JSON fields in `SearchResponse`, `HealthResponse`, or suggestion endpoints.
3. **Altering Storage Formats or Binary Schemas**:
   - Modifying binary serialization layouts, header magic numbers (`MSEDOC01`, `MSEIDX01`), field offsets, or byte encoding protocols for `documents.bin`, `index.bin`, or `dictionary.fst`.

---

## Code Defaults

All implementations must align with the following default practices:

### 1. Explicit Domain Errors Over Silent Fallbacks
- Use strongly-typed Rust error enums or `Result<T, EngineError>` with contextual error descriptions instead of returning silent `None`, empty placeholders, or masking errors.
- Never use `.unwrap()` or `.expect()` in non-test production code without a mathematically proven precondition or explicit boundary check.
- Functions parsing binary disk formats or mmap buffers must return explicit I/O or decode errors upon encountering truncated slices or invalid magic headers.

### 2. Avoid Premature Abstractions for Single-Use Logic
- Prefer straightforward, concrete structs and functions over generalized generics, dynamic dispatch (`dyn Trait`), macro expansions, or layered abstractions when the logic serves a single purpose.
- Only extract traits or interfaces when multiple concrete implementations exist or when required for deterministic testing and dependency inversion.

### 3. Zero Untyped Bypasses & Strict Safety
- Maintain strong type safety across all crate boundaries. Do not use untyped byte arrays or opaque pointer arithmetic where typed structs or slice views can be used.
- Any use of `unsafe` (e.g., `memmap2::Mmap::map`) must be accompanied by an explicit `// SAFETY:` comment justifying why the operation is sound and explaining how preconditions (bounds, alignment, read-only guarantees) are enforced.
- Never commit placeholder code (`todo!()`, `unimplemented!()`, or dummy bypass return values) to production paths.
