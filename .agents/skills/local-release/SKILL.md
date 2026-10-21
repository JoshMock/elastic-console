---
name: local-release
description: Build a Linux x86_64 release binary locally from an exact pushed commit and attach it, with SHA256SUMS and the grammar's LICENSE/NOTICE, to a GitHub release. Use when the release workflow cannot build binaries (missing GRAMMAR_REPO_TOKEN, CI outage) or the user asks for a manual release.
---

# Local release build

Fallback for `.github/workflows/release.yml`. Only `x86_64-unknown-linux-gnu` can be built on the usual dev host; do not install cross toolchains without asking.

## Safety rules

- Creating a release or tag is public. Do it only when the user asks.
- Build from the **pushed** commit and the grammar commit in `GRAMMAR_REF` (`.github/workflows/release.yml`), not from the local working copy or local grammar clone; they can differ.
- Never `--clobber` assets on a published release without the user's go-ahead.

## Workflow

1. Find the version and commit: `Cargo.toml` `version`, `.github/.release-please-manifest.json`, and `gh api repos/JoshMock/elastic-console/commits/main -q .sha`. Check existing tags with `gh release list`.
2. Build and stage assets (sources come from GitHub tarballs, laid out like CI):
   ```sh
   .agents/skills/local-release/scripts/build.sh <app-commit-sha> <grammar-commit-sha>
   ```
   It runs `cargo test --locked` and `cargo build --release --locked`, then writes `/tmp/relassets/` with `elastic-console-linux-amd64`, `SHA256SUMS`, `tree-sitter-elasticsearch-console.LICENSE` and `tree-sitter-elasticsearch-console.NOTICE.txt`. It prints the minimum glibc the binary needs. Put that in the release notes, since the binary won't run on older distros.
3. Create the release or add assets to it:
   ```sh
   gh release create vX.Y.Z -R JoshMock/elastic-console --target <app-commit-sha> --title vX.Y.Z --notes-file notes.md /tmp/relassets/*
   # or, for an existing release:
   gh release upload vX.Y.Z -R JoshMock/elastic-console /tmp/relassets/*
   ```
4. Verify: `gh release download vX.Y.Z -R JoshMock/elastic-console -D /tmp/relcheck && (cd /tmp/relcheck && sha256sum -c SHA256SUMS)`.

## Notes

- In the nono sandbox the script sets `CARGO_HOME=/tmp/cargo-home`, and `/usr/include` must be readable.
- The grammar repo is private; `gh` must be logged in with read access.
