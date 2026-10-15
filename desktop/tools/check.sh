#!/bin/sh
# Repeatable QA against disposable files, databases and mock HTTP services.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"

case "${1:-}" in
  --live)
    # Deliberately separate from normal tests: uses the signed-in subscriber
    # account, refuses token refresh, downloads one track into a temporary folder.
    exec env -u TIBRARY_TEST_MODE cargo test --locked \
      --manifest-path desktop/src-tauri/Cargo.toml \
      bounded_live_download_with_disconnected_cached_library -- --ignored --nocapture
    ;;
  "") ;;
  *) echo "Usage: sh desktop/tools/check.sh [--live]" >&2; exit 2 ;;
esac

export TIBRARY_TEST_MODE=1
npm --prefix desktop test
cargo clippy --locked --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -A clippy::all -D clippy::correctness
cargo test --locked --manifest-path desktop/src-tauri/Cargo.toml
cargo test --locked --manifest-path desktop/src-tauri/Cargo.toml flac_container::tests -- --include-ignored
npm --prefix desktop run build
cargo build --locked --manifest-path desktop/src-tauri/Cargo.toml
npm --prefix desktop run test:e2e
