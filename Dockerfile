# Build Stage
FROM rust:latest AS builder
WORKDIR /app

COPY . .

# Build Crawler, Indexer, and API
RUN cd crawler && cargo build --release
RUN cd indexer && cargo build --release
RUN cd search_api && cargo build --release

# Runtime Stage
FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates bash && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/crawler/target/release/crawler /app/crawler
COPY --from=builder /app/indexer/target/release/indexer /app/indexer
COPY --from=builder /app/search_api/target/release/search_api /app/search_api
COPY --from=builder /app/indexer/dictionary.fst /app/dictionary.fst
COPY docker-entrypoint.sh /app/docker-entrypoint.sh
RUN chmod +x /app/docker-entrypoint.sh

ENV INDEX_PATH=/app/index.bin
ENV DOCS_PATH=/app/documents.bin
ENV FST_PATH=/app/dictionary.fst
ENV RUST_LOG=info

EXPOSE 8080
ENTRYPOINT ["/app/docker-entrypoint.sh"]
