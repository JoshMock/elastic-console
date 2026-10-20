#!/usr/bin/env python3
"""Generate data/body_keys.json from elasticsearch-specification schema.json.

Usage:
    python scripts/gen_body_keys.py /path/to/schema.json
    python scripts/gen_body_keys.py  # downloads from github
"""
import json
import sys
import urllib.request

SCHEMA_URL = "https://raw.githubusercontent.com/elastic/elasticsearch-specification/main/output/schema/schema.json"


def type_id(name_obj):
    """Convert {name, namespace} to 'namespace:name' string."""
    return f"{name_obj['namespace']}:{name_obj['name']}"


def main():
    if len(sys.argv) > 1:
        path = sys.argv[1]
        with open(path) as f:
            data = json.load(f)
    else:
        with urllib.request.urlopen(SCHEMA_URL) as resp:
            data = json.load(resp)

    # Build type index: id -> type definition
    type_idx = {}
    for t in data['types']:
        tid = type_id(t['name'])
        type_idx[tid] = t

    # Build endpoint -> root type mapping (only properties bodies)
    endpoints = {}
    for ep in data['endpoints']:
        req_ns = ep['request']['namespace']
        req_name = ep['request']['name']
        req_id = f"{req_ns}:{req_name}"
        req_type = type_idx.get(req_id)
        if not req_type or 'body' not in req_type:
            continue
        body = req_type['body']
        if body.get('kind') != 'properties':
            continue
        # Create endpoint key like "POST /_search"
        for url in ep.get('urls', []):
            path = url.get('path', '')
            for method in url.get('methods', []):
                key = f"{method} {path}"
                endpoints[key] = req_id

    # Collect reachable types from request bodies
    reachable = set()
    to_visit = list(set(endpoints.values()))

    while to_visit:
        tid = to_visit.pop()
        if tid in reachable:
            continue
        reachable.add(tid)
        t = type_idx.get(tid)
        if not t:
            continue
        # Collect type references
        refs = collect_refs(t)
        for ref in refs:
            if ref not in reachable:
                to_visit.append(ref)

    # Build output types: type id -> {key: child type id or null}
    types = {}
    for tid in reachable:
        t = type_idx.get(tid)
        if not t:
            continue
        entry = resolve_type_entry(t, type_idx, reachable)
        if entry:
            types[tid] = entry

    output = {"endpoints": endpoints, "types": types}
    json_str = json.dumps(output, separators=(',', ':'))

    with open("data/body_keys.json", "w") as f:
        f.write(json_str)

    size_kb = len(json_str) / 1024
    print(f"Wrote {len(endpoints)} endpoints, {len(types)} types to data/body_keys.json ({size_kb:.1f} KB)")


def collect_refs(t):
    """Collect all type references from a type definition."""
    refs = []
    kind = t.get('kind')

    # inherits
    if 'inherits' in t:
        refs.append(type_id(t['inherits']['type']))

    # body (for request types)
    if 'body' in t:
        refs.extend(collect_refs_from_type_ref(t['body']))

    # properties
    for prop in t.get('properties', []):
        refs.extend(collect_refs_from_type_ref(prop.get('type', {})))

    # type_alias target
    if kind == 'type_alias' and 'type' in t:
        refs.extend(collect_refs_from_type_ref(t['type']))

    return refs


def collect_refs_from_type_ref(type_ref):
    """Collect type references from a type reference (instance_of, array_of, etc.)."""
    refs = []
    kind = type_ref.get('kind')

    if kind == 'instance_of':
        if 'type' in type_ref:
            refs.append(type_id(type_ref['type']))
        for g in type_ref.get('generics', []):
            refs.extend(collect_refs_from_type_ref(g))
    elif kind == 'array_of':
        refs.extend(collect_refs_from_type_ref(type_ref.get('value', {})))
    elif kind == 'union_of':
        for item in type_ref.get('items', []):
            refs.extend(collect_refs_from_type_ref(item))
    elif kind == 'dictionary_of':
        refs.extend(collect_refs_from_type_ref(type_ref.get('value', {})))
    elif kind == 'properties':
        for prop in type_ref.get('properties', []):
            refs.extend(collect_refs_from_type_ref(prop.get('type', {})))

    return refs


def resolve_type_entry(t, type_idx, reachable):
    """Resolve a type to its keys -> child type id mapping."""
    kind = t.get('kind')

    if kind == 'type_alias':
        # Follow to target type
        target = resolve_type_ref(t.get('type', {}), type_idx, reachable)
        return target

    if kind in ('interface', 'request'):
        entry = {}
        # Merge parent properties first (inherits)
        if 'inherits' in t:
            parent_id = type_id(t['inherits']['type'])
            parent = type_idx.get(parent_id)
            if parent:
                parent_entry = resolve_type_entry(parent, type_idx, reachable)
                if isinstance(parent_entry, dict):
                    entry.update(parent_entry)

        # Add own properties (for request, from body)
        if kind == 'request' and 'body' in t:
            body = t['body']
            if body.get('kind') == 'properties':
                for prop in body.get('properties', []):
                    name = prop['name']
                    child = resolve_type_ref(prop.get('type', {}), type_idx, reachable)
                    entry[name] = child
                    # Add aliases as keys too
                    for alias in prop.get('aliases', []):
                        entry[alias] = child
        else:
            for prop in t.get('properties', []):
                name = prop['name']
                child = resolve_type_ref(prop.get('type', {}), type_idx, reachable)
                entry[name] = child
                for alias in prop.get('aliases', []):
                    entry[alias] = child

        return entry if entry else None

    return None


def resolve_type_ref(type_ref, type_idx, reachable):
    """Resolve a type reference to a type id or special marker."""
    kind = type_ref.get('kind')

    if kind == 'instance_of':
        tid = type_id(type_ref['type'])
        # Check if it's a builtin or not in reachable
        if tid.startswith('_builtins:') or tid.startswith('_types:'):
            # Check if it resolves to something with keys
            t = type_idx.get(tid)
            if t and t.get('kind') in ('interface', 'type_alias'):
                return tid if tid in reachable else None
            return None
        return tid if tid in reachable else None

    elif kind == 'array_of':
        # Element type - a key inside an array element uses the element type
        return resolve_type_ref(type_ref.get('value', {}), type_idx, reachable)

    elif kind == 'union_of':
        # Merge keys of all object-ish members
        items = type_ref.get('items', [])
        union_keys = {}
        for item in items:
            item_ref = resolve_type_ref(item, type_idx, reachable)
            if item_ref and isinstance(item_ref, str):
                t = type_idx.get(item_ref)
                if t:
                    entry = resolve_type_entry(t, type_idx, reachable)
                    if isinstance(entry, dict):
                        for k, v in entry.items():
                            if k not in union_keys:
                                union_keys[k] = v
        return union_keys if union_keys else None

    elif kind == 'dictionary_of':
        # Dynamic keys (user-defined field names)
        # Mark as "*" -> value type
        value_type = resolve_type_ref(type_ref.get('value', {}), type_idx, reachable)
        return {"*": value_type}

    return None


if __name__ == "__main__":
    main()
