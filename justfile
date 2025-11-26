_:
  just --list

build:
  cargo build --release

test:
  cargo test

lint:
  cargo clippy
