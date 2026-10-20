# elastic-console

A command-line Elasticsearch REPL for the [Elastic CLI](https://github.com/elastic/cli), modeled after the Kibana Dev Tools Console. Type requests like `GET /_search`, press Ctrl+Enter, and see the response. Syntax highlighting and completion are backed by [tree-sitter-elasticsearch-console](https://github.com/elastic/tree-sitter-elasticsearch-console).

**Status:** Experimental. The Elastic CLI is in tech preview. This extension is tested with Elastic CLI 0.7.0 and targets the Elasticsearch API only (Kibana requests are out of scope).

## Install

The Elastic CLI's `elastic extension install` command does not work with this extension because it requires a pre-built binary. Manual installation:

1. Download the binary for your platform from the [GitHub Releases](https://github.com/JoshMock/elastic-console/releases) page.
2. Create `~/.elastic/extensions/elastic-console/` and move the binary there.
3. Rename it to `elastic-console` and make it executable: `chmod +x ~/.elastic/extensions/elastic-console/elastic-console`.
4. Register with the CLI: `elastic extension create console --path ~/.elastic/extensions/elastic-console`.

### Build from source

Requires a C compiler and the grammar repository cloned at `../elastic-neovim/tree-sitter-elasticsearch-console` (path dependency in `Cargo.toml`):

```bash
cargo build --release
elastic extension create console --path ./target/release
```

## Usage

Run `elastic console` with an active Elastic CLI context. The CLI sets the connection environment variables automatically.

### Key bindings

| Key | Action |
|-----|--------|
| Enter | Insert newline |
| Ctrl+Enter | Run request under cursor (or entire buffer if one request) |
| Alt+Enter | Run request (fallback for terminals that don't distinguish Ctrl+Enter) |
| Tab | Open completion menu, or cycle suggestions |
| Enter (in menu) | Accept selected completion |
| Ctrl+C | Clear current input |
| Ctrl+D | Exit |

### History

Command history is saved to `~/.elastic/console_history` (created on first run). Override with `ELASTIC_CONSOLE_HISTORY=/path/to/file`.

### Variables

Define and reference variables in requests:

- `:set name value` – define or overwrite a variable (value is the rest of the line).
- `:vars` – list all variables in alphabetical order.

Variables are substituted before requests are sent:

- `${name}` in paths or JSON bodies → replaced with the variable's value.
- `"${name}"` (the whole JSON string) → if the value is valid JSON (boolean, number, array, object), inserted raw without quotes; otherwise, inserted as a JSON-escaped string.
- `"""${name}"""` (triple-quoted) → always inserted as a JSON-escaped string.
- Undefined variables are left as-is in the request text.

Example:

```
:set idx products
:set filter true
GET /${idx}/_search
{"query": {"match_all": {}}, "filter": "${filter}"}
```

Sends `GET /products/_search` with `"filter": true` (no quotes, raw boolean).

### Piped mode

When stdin is not a TTY, the REPL reads all input, processes `:set`/`:vars` commands, runs all requests in order, and exits:

```bash
printf 'GET /\n' | elastic console
```

Exit codes:
- `0` – all requests completed (including HTTP 4xx/5xx responses).
- `1` – parse error or network failure.

### Completion

- **Methods** – `GET`, `HEAD`, `DELETE`, `POST`, `PUT` prefix-matched at line start.
- **Paths** – 893 endpoint templates from the Elasticsearch API specification (extracted at build time). Path parameters like `{index}` match any typed segment; the last segment is prefix-matched.
- **Index names** – for paths with an `{index}` parameter, live cluster data is fetched via `GET /_resolve/index/*` (indices, aliases, and data streams). Results are cached for 60 seconds. Fetch failures fall back silently to static template matches.

Completion is triggered by Tab. The menu shows live index names first, then static endpoint templates.

## Environment variables

The Elastic CLI sets these automatically from the active context when running `elastic console`:

- `ELASTIC_ES_URL` – Elasticsearch cluster URL (required).
- `ELASTIC_ES_API_KEY` – API key for authentication (if set).
- `ELASTIC_ES_USERNAME` and `ELASTIC_ES_PASSWORD` – basic authentication (if API key is not set).

If you run the `elastic-console` binary directly (outside the CLI), you must export these variables yourself. Missing `ELASTIC_ES_URL` causes a non-zero exit with an error message.

## Development

Run tests:

```bash
cargo test
```

Regenerate the embedded endpoint list (requires `python3`):

```bash
python3 scripts/gen_endpoints.py
```

This downloads the latest API schema from the [elasticsearch-specification](https://github.com/elastic/elasticsearch-specification) repository and writes `data/endpoints.txt`.

See [PLAN.md](PLAN.md) for architecture and implementation notes.

## Limitations

1. **Grammar does not parse `PATCH` requests.** The REPL accepts and sends `PATCH` via the HTTP layer, but the syntax highlighter treats it as an error because the tree-sitter grammar only recognizes `GET`, `HEAD`, `DELETE`, `POST`, and `PUT`.
2. **No `${var}` syntax highlighting.** Variables are substituted at runtime, but the grammar does not highlight them as distinct tokens.
3. **JSON object keys are not highlighted.** The grammar parses keys as hidden nodes, so they appear unstyled.
4. **Bodies must be JSON objects.** The grammar requires request bodies to be `{...}` objects on separate lines (NDJSON for `_bulk` and `_msearch` is supported by sending multiple objects). Arrays or primitives at the top level are not recognized.
5. **No Kibana `kbn:` prefix support.** The REPL ignores `ELASTIC_KIBANA_*` variables and targets the Elasticsearch API only.
6. **Grammar is not published to crates.io.** The dependency is a path reference to a local clone of the grammar repository.

Limitations 1–4 are upstream grammar issues. Contributions to [tree-sitter-elasticsearch-console](https://github.com/elastic/tree-sitter-elasticsearch-console) are welcome.
