# AGENTS.md

`elastic-console`: a Rust REPL that runs as an [Elastic CLI](https://github.com/elastic/cli) extension (`elastic console`), modeled on Kibana Dev Tools Console. User docs: `README.md`.

## Layout

| Path | Purpose |
| --- | --- |
| `src/main.rs` | `Connection::from_env`, `--help`, TTY vs. piped dispatch |
| `src/parse.rs` | tree-sitter parse into `Request { method, path, bodies }`; request-under-cursor selection |
| `src/http.rs` | ureq + rustls client; NDJSON for `_bulk`/`_msearch`; cfg(test) `MockServer` + `read_test_request` |
| `src/repl.rs` | reedline loop, key bindings (`keybindings()`), banner |
| `src/runner.rs` | piped mode |
| `src/vars.rs` | `:set`/`:vars`, `${var}` substitution |
| `src/highlight.rs` | `reedline::Highlighter` from the grammar's `HIGHLIGHTS_QUERY`; also records the cursor offset |
| `src/complete.rs` | `reedline::Completer`: methods, paths, live index names, body keys, field names |
| `src/index_source.rs`, `src/mapping_source.rs` | `_resolve/index/*` and `_mapping` fetches, 60 s cache, 2 s timeout |
| `src/body_keys.rs` | walks `data/body_keys.json` along the JSON path at the cursor |
| `data/` | generated from elasticsearch-specification; regenerate, don't hand-edit (skill: `regenerate-spec-data`) |

## Build and test

- The grammar is a **path dependency** on `../elastic-neovim/tree-sitter-elasticsearch-console` (private repo `elastic/tree-sitter-elasticsearch-console`). It must be cloned there. CI checks it out at `GRAMMAR_REF` (keep `ci.yml` and `release.yml` in sync).
- It compiles C, so a C compiler and readable system headers are required.
- `cargo test` and `cargo clippy --all-targets` must both be clean before committing.
- In the nono sandbox: `export CARGO_HOME=/tmp/cargo-home` (`~/.cargo/registry` is read-only), and `/usr/include` must be readable or the grammar build fails with `stdc-predef.h: Permission denied`.
- Interactive behaviour (key bindings, menus, colours) cannot be tested without a TTY. Unit-test what you can (e.g. `keybindings().find_binding(..)`), then ask the user to check in a real terminal.

## Testing conventions

- Red-green TDD: write the test, see it fail, then implement.
- After writing a test, break the implementation on purpose and confirm the test fails. Several tests in this repo passed against broken code before this check was done.
- HTTP tests use `http::MockServer`. Read requests with `read_test_request`: a single `read()` loses bodies that arrive in a later packet and makes tests flaky.
- Tests must never touch `$HOME` (history path is injected).

## Gotchas

- **Grammar shape:** `_object` and object keys are hidden rules. A body's direct children are its leaf values and brace tokens, not objects. Split bodies on top-level `{`/`}` tokens (see `parse.rs`). Strings have no escape support, so substitute `${var}` **after** parsing, never before.
- **Grammar gaps:** no `PATCH`, no `${var}` node, no named key node. Body-key and field completion therefore use a text scan for key position and JSON path.
- **Highlighting:** skip the `@none` capture on `(body)`; it spans the whole body and masks the JSON captures.
- **reedline:** there is no cursor position on Submit. The highlighter records the cursor on every repaint, and `submit_buffer()` repaints just before returning. Enter is `UntilFound([MenuAccept, Edit(InsertNewline)])` so it accepts an open completion menu.
- **Elastic CLI extension contract:** the CLI scrubs the environment (no `XDG_*`), passes `ELASTIC_ES_URL` plus `ELASTIC_ES_API_KEY` or `ELASTIC_ES_USERNAME`/`PASSWORD`, and spawns the binary with inherited stdio. `elastic console --help` shows the root CLI help (elastic/cli#795); run the binary directly to see its help. `elastic extension install` cannot install release binaries (elastic/cli#797); installs are manual.

## Version control and releases

- This is a jj repo. Use `jj`, never `git`.
- Commit messages must follow Conventional Commits (`feat:`, `fix:`, `chore:`, `ci:`, `docs:` …). release-please derives versions and `CHANGELOG.md` from them; other messages are ignored. Add an `Assisted-by:` trailer.
- `main` is protected by a ruleset: changes land through PRs, linear history, no force-push. Push a bookmark and open a PR.
- Pushing changes to `.github/workflows/*` over HTTPS needs a token with the `workflow` scope. If `gh`'s token lacks it, ask the user to push over SSH.
- Inside the nono sandbox SSH push fails (`~/.ssh/known_hosts` is denied). Push via the `https` remote with `gh` credentials:
  `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=credential.https://github.com.helper GIT_CONFIG_VALUE_0='!gh auth git-credential' jj git push --remote https -b <bookmark>`
  Do not `jj git remote remove` a remote you pushed through: it once abandoned the pushed commits locally (recovered with `jj op restore`).
- Releases: `.github/workflows/release.yml` (release-please, then 5 platform builds, then `SHA256SUMS` + grammar `LICENSE`/`NOTICE`). It needs the `GRAMMAR_REPO_TOKEN` secret. For a manual Linux build, use the `local-release` skill.
- Licensing: MIT. Release binaries bundle the Apache-2.0 grammar, so its `LICENSE` and `NOTICE.txt` go on every release.
