# Default: list available commands
_:
  just --list

# Check code formatting
format-check:
  treefmt --ci

# Apply code formatting
format:
  treefmt

# Run clippy lints
lint:
  cargo clippy --workspace --quiet

# Build all crates (debug)
build:
  cargo build --workspace --quiet

# Build all crates (release)
build-release:
  cargo build --workspace --release

# Run unit tests
unit-test:
  cargo nextest run --workspace --show-progress=none --status-level=fail --cargo-quiet
  cargo test --workspace --doc --quiet

# Run all checks (format, lint, build, test) - continues on failure, exits non-zero if any failed
check:
  #!/usr/bin/env bash
  failed=0
  echo "=== Format Check ==="
  just format-check || failed=1
  echo ""
  echo "=== Lint ==="
  just lint || failed=1
  echo ""
  echo "=== Build ==="
  just build || failed=1
  echo ""
  echo "=== Unit Tests ==="
  just unit-test || failed=1
  echo ""
  if [ $failed -eq 1 ]; then
    echo "=== SOME CHECKS FAILED ==="
    exit 1
  else
    echo "=== ALL CHECKS PASSED ==="
  fi
