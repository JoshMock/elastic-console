#!/usr/bin/env bash
# Usage: build.sh <app-commit-sha> <grammar-commit-sha>
# Builds elastic-console for x86_64 Linux from GitHub tarballs and stages release assets in /tmp/relassets.
set -euo pipefail

app_sha=${1:?app commit sha}
grammar_sha=${2:?grammar commit sha}
root=/tmp/relbuild
out=/tmp/relassets
export CARGO_HOME=${CARGO_HOME:-/tmp/cargo-home}

rm -rf "$root" "$out"
mkdir -p "$root/elastic-repl" "$root/elastic-neovim/tree-sitter-elasticsearch-console" "$out"

gh api "repos/JoshMock/elastic-console/tarball/$app_sha" | tar -xz -C "$root/elastic-repl" --strip-components=1
gh api "repos/elastic/tree-sitter-elasticsearch-console/tarball/$grammar_sha" \
  | tar -xz -C "$root/elastic-neovim/tree-sitter-elasticsearch-console" --strip-components=1

cd "$root/elastic-repl"
cargo test --locked
cargo build --release --locked

cd "$out"
cp "$root/elastic-repl/target/release/elastic-console" elastic-console-linux-amd64
cp "$root/elastic-neovim/tree-sitter-elasticsearch-console/LICENSE" tree-sitter-elasticsearch-console.LICENSE
cp "$root/elastic-neovim/tree-sitter-elasticsearch-console/NOTICE.txt" tree-sitter-elasticsearch-console.NOTICE.txt
sha256sum elastic-console-* > SHA256SUMS

echo "min glibc: $(objdump -T elastic-console-linux-amd64 | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1)"
ls -l "$out"
