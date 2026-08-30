# Build Stage
FROM rust:latest AS builder
WORKDIR /app

COPY . .

# Build Crawler, Indexer, and API
RUN cd crawler && cargo build --release
RUN cd indexer && cargo build --release
RUN cd search_api && cargo build --release

# Crawl & Index inside container build
RUN cd crawler && ./target/release/crawler
RUN cd indexer && ./target/release/indexer

# Runtime Stage
FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/search_api/target/release/search_api /app/search_api
COPY --from=builder /app/indexer/index.json /app/index.json
COPY --from=builder /app/crawler/documents.json /app/documents.json

ENV INDEX_PATH=/app/index.json
ENV DOCS_PATH=/app/documents.json
ENV RUST_LOG=info

EXPOSE 8080
CMD ["/app/search_api"]
