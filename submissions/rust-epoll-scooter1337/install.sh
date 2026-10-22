#!/usr/bin/env bash
set -euo pipefail
apt-get update
apt-get install -y build-essential curl ca-certificates
# A system toolchain permits build.sh to run as the unprivileged benchmark user.
export RUSTUP_HOME=/opt/twelve-rust/rustup CARGO_HOME=/opt/twelve-rust/cargo
curl -fsSL https://sh.rustup.rs -o /tmp/twelve-rustup.sh
sh /tmp/twelve-rustup.sh -y --profile minimal --default-toolchain 1.94.0 --no-modify-path
chmod -R a+rX /opt/twelve-rust
for tool in cargo rustc; do
  printf '#!/bin/sh\nexport RUSTUP_HOME=/opt/twelve-rust/rustup\nexec /opt/twelve-rust/cargo/bin/%s "$@"\n' "$tool" > "/usr/local/bin/$tool"
  chmod 755 "/usr/local/bin/$tool"
done
