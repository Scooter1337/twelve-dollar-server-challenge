#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
if [[ -x /opt/twelve-rust/cargo/bin/cargo ]]; then
  export RUSTUP_HOME=/opt/twelve-rust/rustup
  export PATH="/opt/twelve-rust/cargo/bin:$PATH"
else
  export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi
export RUSTUP_TOOLCHAIN=1.94.0
mkdir -p vendor
if [[ ! -f vendor/sqlite3.c ]]; then
  curl -fsSL --retry 3 https://www.sqlite.org/2026/sqlite-autoconf-3530400.tar.gz -o vendor/sqlite.tar.gz
  printf '%s  %s\n' 0e9483900e92cd5de8fd48d16bf9200145a61f7fd5be542a5ac81d8a9516eb9c vendor/sqlite.tar.gz | shasum -a 256 -c -
  tar -xzf vendor/sqlite.tar.gz -C vendor --strip-components=1 sqlite-autoconf-3530400/sqlite3.c sqlite-autoconf-3530400/sqlite3.h
  rm vendor/sqlite.tar.gz
fi
export RUSTFLAGS="-C target-cpu=native ${RUSTFLAGS:-}"
cargo build --release --locked "$@"
