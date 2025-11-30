_:
  just --list

build:
  cargo build --release --workspace

test:
  cargo test --workspace

lint:
  cargo clippy --workspace
