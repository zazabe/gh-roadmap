#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> fmt"
cargo fmt --all -- --check

echo "==> clippy"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> test"
cargo test --all

echo "ok"
