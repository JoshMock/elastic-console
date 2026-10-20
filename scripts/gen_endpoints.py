#!/usr/bin/env python3
"""Generate data/endpoints.txt from elasticsearch-specification schema.json.

Usage:
    python scripts/gen_endpoints.py /path/to/schema.json
    python scripts/gen_endpoints.py  # downloads from github
"""
import json
import sys
import urllib.request

SCHEMA_URL = "https://raw.githubusercontent.com/elastic/elasticsearch-specification/main/output/schema/schema.json"

def main():
    if len(sys.argv) > 1:
        path = sys.argv[1]
        with open(path) as f:
            data = json.load(f)
    else:
        with urllib.request.urlopen(SCHEMA_URL) as resp:
            data = json.load(resp)

    info = data.get("_info", {})
    title = info.get("title", "")

    # Collect (method, path) pairs
    pairs = set()
    for ep in data.get("endpoints", []):
        for url in ep.get("urls", []):
            path = url.get("path", "")
            for method in url.get("methods", []):
                pairs.add((method, path))

    lines = sorted(f"{m} {p}" for m, p in pairs)

    with open("data/endpoints.txt", "w") as f:
        f.write(f"# Generated from: {title}\n")
        f.write(f"# Endpoints: {len(lines)}\n")
        for line in lines:
            f.write(line + "\n")

    print(f"Wrote {len(lines)} endpoints to data/endpoints.txt")

if __name__ == "__main__":
    main()
