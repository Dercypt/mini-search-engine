#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

CRATES=("crawler" "indexer" "search_api")

echo "=================================================="
echo " Starting Verification Pipeline (mini-search-engine)"
echo "=================================================="

# 1. Code Formatting Check
echo ""
echo "=== Stage 1: Checking Code Formatting (rustfmt) ==="
for crate in "${CRATES[@]}"; do
  echo "--> Checking formatting in $crate..."
  cargo fmt --manifest-path "$crate/Cargo.toml" --check
done

# 2. Static Analysis & Lints
echo ""
echo "=== Stage 2: Static Analysis & Lints (clippy) ==="
for crate in "${CRATES[@]}"; do
  echo "--> Running clippy in $crate..."
  cargo clippy --manifest-path "$crate/Cargo.toml" -- -D warnings
done

# 3. Unit and Integration Tests
echo ""
echo "=== Stage 3: Running Unit and Integration Tests ==="
for crate in "${CRATES[@]}"; do
  echo "--> Running tests in $crate..."
  cargo test --manifest-path "$crate/Cargo.toml"
done

# 4. Law Verification Tests (if directory exists)
echo ""
echo "=== Stage 4: Law Verification Tests ==="
if [ -d "tests/laws" ]; then
  echo "--> Verifying tests/laws invariants..."
  if [ -f "tests/laws/Cargo.toml" ]; then
    cargo test --manifest-path "tests/laws/Cargo.toml"
  else
    cargo test --test laws
  fi
else
  echo "--> tests/laws directory not present. Skipping law test suite."
fi

echo ""
echo "=================================================="
echo " All verification checks passed successfully!"
echo "=================================================="
