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
  cargo clippy --workspace --all-targets --quiet

# Enforce per-file line budgets (see scripts/file-size-budgets.txt)
size-check:
  ./scripts/check-file-sizes.sh

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
  echo "=== File Sizes ==="
  just size-check || failed=1
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

# Run the incremental-compilation oracle suites: re-run the cache tests with the
# recompile-and-compare oracle forced on (`AMBIENT_CACHE_VERIFY`/
# `AMBIENT_ANALYSIS_VERIFY`), which recompiles every module on a memo/cache hit
# and panics if a served result disagrees with a fresh compute. This is the
# strongest check that incremental compilation never serves a stale result.
#
# Deliberately NOT part of `just check`: it re-runs suites `check` already ran,
# just under the oracle env, so it roughly doubles their runtime. Run it before
# landing changes to the build cache or the analysis session.
check-oracles:
  #!/usr/bin/env bash
  failed=0
  echo "=== Build Cache Oracle (AMBIENT_CACHE_VERIFY=1) ==="
  AMBIENT_CACHE_VERIFY=1 cargo test -p ambient-cli --test incremental_cache --test incremental_cache_deps --quiet || failed=1
  echo ""
  echo "=== Analysis Oracle (AMBIENT_ANALYSIS_VERIFY=1) ==="
  AMBIENT_ANALYSIS_VERIFY=1 cargo test -p ambient-analysis --quiet || failed=1
  echo ""
  if [ $failed -eq 1 ]; then
    echo "=== SOME ORACLE CHECKS FAILED ==="
    exit 1
  else
    echo "=== ALL ORACLE CHECKS PASSED ==="
  fi
