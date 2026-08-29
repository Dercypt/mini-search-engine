#!/usr/bin/env bash
set -e

echo "=== [1/3] Running Concurrent Web Crawler ==="
cd crawler
cargo run --release
cd ..

echo -e "\n=== [2/3] Building Inverted Index & BM25 Cache ==="
cd indexer
cargo run --release
cd ..

echo -e "\n=== [3/3] Starting Search API & Web UI on http://localhost:8080 ==="
cd search_api
cargo run --release
