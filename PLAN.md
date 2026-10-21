# elastic-repl: implementation plan

A Rust REPL that runs as an [Elastic CLI](https://github.com/elastic/cli) extension (`elastic console`; repo and binary `elastic-console`). It works like Kibana Dev Tools Console: type `GET /`, press `Enter`, see the response. Syntax highlighting comes from [tree-sitter-elasticsearch-console](https://github.com/elastic/tree-sitter-elasticsearch-console).

Background: [llm-wiki spacetime/neovim-plugin.md, 2026-09 update](https://github.com/JoshMock/llm-wiki/blob/main/spacetime/neovim-plugin.md).

## Constraints from the CLI extension contract

Source: `elastic/cli` [README#extensions](https://github.com/elastic/cli#extensions) and `src/extension/*.ts`.

1. **Naming.** Repo/package `elastic-<name>` → command `elastic <name>`. Name must match `[a-z0-9-]+` (`installer.ts`).
2. **Process model.** The CLI spawns the entrypoint with `stdio: 'inherit'`, `shell: false`, and forwards the exit code (`runner.ts`). An interactive TTY REPL works.
3. **Credentials come from env vars** (`context.ts`): `ELASTIC_ES_URL` plus either `ELASTIC_ES_API_KEY` or `ELASTIC_ES_USERNAME`/`ELASTIC_ES_PASSWORD`. The README only lists the API-key vars; support both. Same pattern for `ELASTIC_KIBANA_*` and `ELASTIC_CLOUD_*`. Treat an absent var as "not configured". If `ELASTIC_ES_URL` is unset, exit non-zero with `run via 'elastic console' or set ELASTIC_ES_URL` (no localhost fallback); exporting the vars by hand also works.
4. **Scrubbed env** (`env.ts`): only `PATH`, `HOME`, `TERM`, locale, `TZ`, proxy vars, `NODE_EXTRA_CA_CERTS`, and `SSL_CERT_*` pass through. So `XDG_*` is **not** passed: put history/config under `$HOME`. Honour `SSL_CERT_FILE`/`HTTPS_PROXY` in the HTTP client.
5. **Install has no build step** (`installer.ts`):
   - GitHub source: `git clone --depth 1`, then `npm install --production --ignore-scripts` if `package.json` exists. Entrypoint = `package.json` `bin`, or an executable named `elastic-<name>` in the root, `bin/`, or `dist/`.
   - npm source: `npm install --prefix … --ignore-scripts`, entrypoint from `node_modules/.bin`.
   - `--ignore-scripts` means **no `postinstall` download of a binary**. `cargo build` never runs.
   - The entrypoint must resolve (realpath) inside the install dir.
6. **Discovery.** `elastic extension search` finds repos with the GitHub topic `elastic-extension` (`search.ts`).
7. **Tech preview.** The CLI is at v0.7.0 (2026-10-15 release). Pin a tested version in the README.

## Architecture

One Rust binary, a few modules. No workspace until something needs to be shared.

| Module | Job | Crate |
| --- | --- | --- |
| `env` | Read `ELASTIC_*` vars into a `Connection` | stdlib |
| `parse` | Buffer → list of `Request { method, path, bodies }` via tree-sitter | `tree-sitter`, `tree-sitter-elasticsearch-console` (git dep, not on crates.io) |
| `http` | Send request, return status + headers + body | `ureq` + rustls (simpler API than reqwest, pure Rust TLS) |
| `repl` | Line editor, history, key bindings | `reedline` 0.52 |
| `highlight` | `reedline::Highlighter` backed by the grammar's `highlights.scm` | `tree-sitter-highlight` |
| `complete` | `reedline::Completer` | see phases |
| `output` | Print status line + pretty JSON to stdout below the request, then a new prompt (shell-style scrollback, no TUI panes) | `serde_json` |

Why `reedline`: it has the needed traits: `Highlighter`, `Completer`, `Hinter`, `History`, `Validator` ([docs.rs](https://docs.rs/reedline/latest/reedline/)).

### Key bindings (decision: Kibana-style)

- **Enter** always inserts a newline.
- **Ctrl+Enter** sends the request under the cursor (or the whole buffer if it holds one request).
- Implement with a custom `Keybindings` entry mapping Ctrl+Enter to `Submit`, and Enter to `InsertNewline`. The `Validator` is then only used to refuse submission of a buffer that does not parse (show the error, keep the buffer).
- **Risk:** many terminals send the same byte for Enter and Ctrl+Enter. Reedline can enable the kitty keyboard protocol with `use_kitty_keyboard_enhancement(true)` and check support with `kitty_protocol_available` ([Reedline docs](https://docs.rs/reedline/latest/reedline/struct.Reedline.html)). Phase 1 must test this in the target terminals and provide a fallback binding (e.g. Alt+Enter or Ctrl+J) that works everywhere.

## Phases

### Phase 0: skeleton ✅ done (verified via `elastic console`)

1. `cargo new elastic-console`; binary prints the connection info (like the [demo extension](https://github.com/elastic/cli/tree/main/examples/extensions/basic)).
2. Register locally: `elastic extension create console --path ./target/release`  (`--path` registers an existing dir without overwriting).
3. Verify: `elastic console` prints the URL from the active context.

### Phase 1: MVP REPL ✅ done (verified in a real terminal: Ctrl+Enter and Alt+Enter both submit; 200 OK against a live cluster)

1. `reedline` loop with file history at `~/.elastic/console_history`.
2. Key bindings from "Key bindings" above, including the fallback binding.
3. On submit: parse, send, pretty-print. Body lines for `_bulk`/`_msearch` are sent as NDJSON (`Content-Type: application/x-ndjson`); otherwise `application/json`.
4. Auth header: `Authorization: ApiKey <key>` or basic auth.
5. `Ctrl-C` cancels the current input, `Ctrl-D` exits.
6. Non-TTY mode: if stdin is not a TTY, read all requests from stdin, run them in order, print results. This is what an editor wrapper (Neovim/Snacks) sends a buffer to.
7. Tests: parser → `Request` unit tests; HTTP against a mock server (`httpmock` or a tiny `std::net` listener).

**Implementation notes:**
- HTTP client: `ureq` + rustls instead of `reqwest`. Simpler API for blocking HTTP, pure Rust TLS via ring crypto and webpki roots.
- History path injectable via `ELASTIC_CONSOLE_HISTORY` env var (for testing without touching `$HOME`).
- Piped mode exit codes: 0 = all requests completed (including HTTP 4xx/5xx), 1 = parse error or network error.
- Run-under-cursor implemented: reedline's `Highlighter::highlight(line, cursor)` is called with cursor position on every repaint, including the final repaint during `submit_buffer`. Store cursor in shared `Arc<AtomicUsize>`, read after `Signal::Success`, use `select_request_at_cursor(buffer, cursor)` to pick the request. Behaviour: cursor inside request → that request; between requests or on blank line → nearest preceding; no preceding → first. Single request in buffer: unchanged. Piped mode: runs all (unchanged).
- Kitty keyboard enhancement enabled (`use_kitty_keyboard_enhancement(true)`). Alt+Enter works as fallback.

**Dev environment:**
- The grammar is a path dependency on a local clone at `../elastic-neovim/tree-sitter-elasticsearch-console`. In the nono sandbox, build with `CARGO_HOME=/tmp/cargo-home` (`~/.cargo/registry` is read-only) and grant read on `/usr/include` for the C compiler.

### Phase 2: syntax highlighting ✅ done (colours verified in a real terminal)

1. Load `queries/elasticsearch_console/highlights.scm` from the grammar crate.
2. Map capture names (`@keyword`, `@function`, `@string`, `@number`, …) to `nu_ansi_term` styles; return a `StyledText` from `Highlighter::highlight`.
3. Re-parse on each keystroke. Buffers are small; incremental parsing is not needed yet.

**Implementation notes:**
- Used direct `tree_sitter::Query` + `QueryCursor` approach (shorter than tree-sitter-highlight crate).
- `SyntaxHighlighter` implements `reedline::Highlighter`.
- Parser wrapped in `RefCell` for interior mutability (&self requirement).
- Maps captures: keyword→cyan bold, function/path→yellow, string→green, number→purple, comment→dark gray.
- Handles incomplete/invalid input gracefully (ERROR nodes while typing).
- Red-green TDD: tests verify text preservation and styling for method/path segments.
- Highlights method, path, comments, and JSON values/punctuation. Object keys stay unstyled: the grammar has no named node for them (grammar gap 3).

### Phase 3: simple autocomplete ✅ done (Tab menu + Enter-to-accept verified in a real terminal)

Static, offline, no cluster round trip.

1. **Methods** at line start: `GET HEAD DELETE POST PUT`.
2. **Paths** (decision: one embedded list from spec `main`, stack schema only; no serverless/version switching): build a list from the [elasticsearch-specification](https://github.com/elastic/elasticsearch-specification) `output/schema/schema.json`. Each endpoint has `urls[].methods` and `urls[].path` (e.g. `/_internal/desired_balance`). Strip it down at build time to `(method, path-template)` pairs and embed with `include_str!`. The full schema is ~11 MB; the stripped list is small.
3. Completion uses the tree-sitter node under the cursor (`method` vs `path`) to decide what to offer.

**Implementation notes:**
- `scripts/gen_endpoints.py` reads schema.json (arg or downloads) → `data/endpoints.txt` (893 endpoints, 28KB).
- `src/complete.rs`: `EndpointCompleter` implementing `reedline::Completer`.
- Plain text context detection (heuristic: body starts with `{`, `[`, or whitespace+non-uppercase).
- Path matching segment-wise: `{param}` matches any typed non-empty segment; last segment prefix-matched.
- Tab opens ColumnarMenu; keybinding `UntilFound([Menu("completion_menu"), MenuNext])`.
- 12 unit tests covering method prefix, path completion, param substitution, method filtering, span correctness, querystring/body exclusion, multiline buffers.

### Phase 3.5: `${var}` variables ✅ done (verified in a real terminal)

Decision: no grammar change needed. The grammar already parses `${x}` as part of a path segment or a JSON string (verified: `GET /${idx}/_search`, `"""${shopName}"""`, `"${pin}"` all parse). Substitute in the request text before sending. A named `variable` node for highlighting is a later, optional grammar PR.

Substitution rules, from the [Kibana Console docs](https://www.elastic.co/docs/explore-analyze/query-filter/tools/console):

1. Variables are allowed in paths and bodies, and may be embedded inside a longer string.
2. `"${var}"` (the whole string): substituted without the quotes if the value is a boolean, number, array, or object.
3. `"""${var}"""`: always substituted as a string.
4. An empty value is substituted as `""`.

Defining variables (decision): REPL commands, kept in memory for the session only.

- `:set name value`: define or overwrite. `value` is the rest of the line, taken literally; strings are not quoted (`:set idx logs`, not `:set idx "logs"`).
- `:vars`: list all variables.
- An undefined `${name}` is left as-is, matching Kibana ("replaced … if the variable exists").
- Piped mode (decision): `:set` lines are processed in order with requests, so a script can define its own variables.

### Phase 4 (stretch): LSP-like completion

1. **Live cluster data** ✅ done (verified in a real terminal) (decisions): index, alias, and data stream names from one call, `GET /_resolve/index/*` ([docs](https://www.elastic.co/docs/api/doc/elasticsearch/operation/operation-indices-resolve-index)). Default `expand_wildcards=open`, so hidden/system indices are excluded. Fetched lazily on the first Tab inside an `{index}` slot, then cached for 60 s. Only `{index}` params are filled; `{name}` is too ambiguous (aliases, templates, pipelines, …). A failed fetch falls back silently to the static template list (also caches empty for 60 s to avoid retry storm).
   - `src/index_source.rs`: fetch, parse, 60 s cache with injected clock for testing.
   - `src/complete.rs`: `with_connection(conn)` for live mode; detects `{index}` slot in templates.
   - Ordering: live index names first, then static template matches (real indices outrank expansions of a partial name like `/book/_count`).
   - 14 tests (8 index_source + 6 completer integration).
2. **Field names** ✅ code done, pending manual TTY check: suggest field names from `GET <index>/_mapping` while typing a JSON object KEY inside a request body.
   - Index = first path segment if it does not start with `_` (e.g. `POST /books/_search` → `books`; `/_search` → no field suggestions). Comma lists / wildcards passed as-is to `GET /<segment>/_mapping`.
   - `src/mapping_source.rs`: fetch, parse, 60 s cache per index with injected clock for testing, 2 s timeout. Flattens `properties` recursively into dotted paths (`author.name`), includes multi-fields (`title.keyword`). Failures cached as empty.
   - Trigger: cursor in JSON string in KEY position (after `{` or `,`, before `:`). Not in triple-quoted strings. Text-based scan (grammar has no named key node).
   - Completion value = field name only (no quotes). Span = typed prefix after opening quote.
   - 9 mapping_source tests + 9 completer integration tests.
3. **Body keys** ✅ code done, pending manual TTY check: suggest valid keys for the cursor's JSON path from elasticsearch-specification types.
   - Decision: text-based JSON path scan, no grammar changes (grammar has no named key node, but text scan works).
   - `scripts/gen_body_keys.py`: generates `data/body_keys.json` from schema.json (364 endpoints, 1251 types, 365KB).
   - `src/body_keys.rs`: loads spec via `include_str!` + `serde_json` (lazy `OnceLock`). `keys_at(endpoint_key, json_path)` walks types; `json_path_at_cursor(text)` scans body text.
   - Handles `inherits` (merge parent), `type_alias` (follow), `array_of` (transparent), `union_of` (merge keys), `dictionary_of` (mark as `"*"` → field names only).
   - Completer: match method+path to endpoint key, compute JSON path, offer spec keys (prefix match) first, then field names, dedupe. At `"*"` level, only field names.
   - 21 tests (15 body_keys + 6 completer integration).
4. Split this into a library crate so the same engine can back an LSP server later ([elasticsearch-lsp idea](https://github.com/JoshMock/llm-wiki/blob/main/spacetime/elasticsearch-lsp.md)).

## Grammar gaps (upstream PRs to the grammar repo)

Read from [`grammar.js`](https://github.com/elastic/tree-sitter-elasticsearch-console/blob/main/grammar.js); the README calls the grammar "very experimental".

1. No `PATCH` method.
2. No `${var}` variable interpolation.
3. Object keys are hidden rules (`_object_key_val`, `_string_single`), so keys cannot be highlighted. However, Phase 4 body key completion works via text-based JSON path scan, no grammar change needed.
4. Bodies must be objects (`body: repeat1(seq($._object, '\n'))`).
5. No `kbn:` prefix for Kibana API requests ([Console docs](https://www.elastic.co/docs/explore-analyze/query-filter/tools/console)). Out of scope for now: the REPL targets Elasticsearch only and ignores `ELASTIC_KIBANA_*`.
6. Not published to crates.io; use a git dependency pinned to a commit.

## Distribution

Decision: build binaries by hand and attach them to GitHub Releases.

Caveat: `elastic extension install owner/repo` clones the repo and does not read Releases (constraint 5), so it cannot install this. Install is manual:

1. Download the binary for your platform from the Releases page into a dir, e.g. `~/.elastic/extensions/elastic-console/`.
2. Name it `elastic-console` and `chmod +x` it.
3. `elastic extension create console --path ~/.elastic/extensions/elastic-console` (discovers the existing executable, does not overwrite it).

Upgrade = repeat steps 1–2. Revisit npm per-platform packages or CI-built releases if manual installs become a pain.

## Open questions

All answered; decisions are folded in above.

1. ~~Distribution~~: manual builds on GitHub Releases (answered).
2. ~~Name~~: `console` → `elastic console`, repo `elastic-console` (answered).
3. ~~Enter behaviour~~: Enter = newline, Ctrl+Enter = run, like Kibana (answered).
4. ~~Response display~~: inline, shell-style scrollback; no split-pane TUI (answered).
5. ~~`${var}`~~: deferred to phase 3.5, after the upstream grammar PR (answered).
6. ~~Completion spec source~~: embedded `schema.json` from spec `main` only (answered).
7. ~~`kbn:` Kibana requests~~: out of scope; Elasticsearch only (answered).
8. ~~Piped mode~~: in MVP, phase 1 step 6 (answered).
9. ~~Missing `ELASTIC_ES_URL`~~: exit with an error, no fallback (answered).
