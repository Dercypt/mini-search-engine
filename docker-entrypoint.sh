#!/usr/bin/env bash
set -e

# Run pipeline if index or documents do not exist
if [ ! -f "${INDEX_PATH:-/app/index.bin}" ] || [ ! -f "${DOCS_PATH:-/app/documents.bin}" ]; then
    echo "Index or document store not found. Running crawler and indexer pipeline..."
    if [ ! -f "${DOCS_PATH:-/app/documents.bin}" ]; then
        echo "--> Crawling seed pages..."
        /app/crawler --max-pages 50 --output /app/documents.json
    fi
    if [ ! -f "${INDEX_PATH:-/app/index.bin}" ]; then
        echo "--> Building inverted index and lexicon..."
        /app/indexer
    fi
fi

echo "--> Starting Search API..."
exec /app/search_api
