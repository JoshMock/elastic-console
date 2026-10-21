---
name: regenerate-spec-data
description: Regenerate data/endpoints.txt and data/body_keys.json from the elasticsearch-specification schema. Use when path or body-key completion is stale, when a new Elasticsearch API should complete, or after changing scripts/gen_endpoints.py or scripts/gen_body_keys.py.
---

# Regenerate spec-derived completion data

`data/endpoints.txt` (method + path templates) and `data/body_keys.json` (request-body key trees) are generated from `output/schema/schema.json` in [elasticsearch-specification](https://github.com/elastic/elasticsearch-specification) (`main` branch, stack schema only). They are embedded with `include_str!`. Never hand-edit them.

## Workflow

1. Download the schema once (~11 MB) so both generators use the same copy:
   ```sh
   curl -sL -o /tmp/schema.json https://raw.githubusercontent.com/elastic/elasticsearch-specification/main/output/schema/schema.json
   ```
2. Regenerate both files:
   ```sh
   python3 scripts/gen_endpoints.py /tmp/schema.json
   python3 scripts/gen_body_keys.py /tmp/schema.json
   ```
   Without an argument each script downloads the schema itself.
3. Check the size and shape of the diff (`jj diff --stat data/`). `body_keys.json` should stay well under 1 MB. A large unexpected change usually means the spec's type shapes changed. Inspect them with small `python3 -c` one-liners against `/tmp/schema.json`; don't print the file.
4. Run `cargo test`. The `body_keys` and `complete` tests assert real keys (`query`, `bool`, `must`, `/_cat/indices`, …), so a breaking spec change shows up there.
5. Commit generator changes and data changes together as `chore: regenerate spec completion data` (or `feat:` if new completions are user-visible).

## Notes

- `schema.json` type kinds: `interface` (with `inherits`), `type_alias`, `enum`, `request`, `response`. Value kinds: `instance_of`, `array_of`, `union_of`, `dictionary_of`, `user_defined_value`, `literal_value`. `dictionary_of` is stored as a `"*"` key (field names are offered there instead of spec keys).
- Only `{index}` path params are filled with live index names; other `{param}`s are completed as literal template text.
