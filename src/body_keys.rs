//! Body keys from elasticsearch-specification.
//!
//! Provides spec-based key suggestions for JSON request bodies.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

static DATA: OnceLock<BodyKeysData> = OnceLock::new();

struct BodyKeysData {
    endpoints: HashMap<String, String>,
    types: HashMap<String, Value>,
}

fn load_data() -> &'static BodyKeysData {
    DATA.get_or_init(|| {
        let json = include_str!("../data/body_keys.json");
        let v: Value = serde_json::from_str(json).expect("body_keys.json parse");
        let endpoints = v["endpoints"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
            .unwrap_or_default();
        let types = v["types"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        BodyKeysData { endpoints, types }
    })
}

/// Get valid keys at the given JSON path for an endpoint.
/// Path is the sequence of object keys leading to the current position.
/// Arrays are transparent (stepping through an array uses the element type).
/// Returns empty vec for unknown endpoints or at dictionary ("*") levels.
pub fn keys_at(endpoint_key: &str, path: &[String]) -> Vec<String> {
    let data = load_data();

    // Look up root type for this endpoint
    let Some(root_type_id) = data.endpoints.get(endpoint_key) else {
        return vec![];
    };

    // Start at the root type
    let Some(root) = data.types.get(root_type_id) else {
        return vec![];
    };

    // Walk the path
    let mut current = root.clone();
    for key in path {
        current = match step_into(&current, key, &data.types) {
            Some(v) => v,
            None => return vec![], // Path not found
        };
    }

    // Return keys at current level
    keys_from_value(&current, &data.types)
}

/// Step into a type by key name.
fn step_into(value: &Value, key: &str, types: &HashMap<String, Value>) -> Option<Value> {
    match value {
        Value::Object(map) => {
            // Check for exact key
            if let Some(v) = map.get(key) {
                return Some(resolve_ref(v, types));
            }
            // Check for wildcard (dictionary)
            if let Some(v) = map.get("*") {
                return Some(resolve_ref(v, types));
            }
            None
        }
        Value::String(type_id) => {
            // Resolve type reference, then step into
            let resolved = types.get(type_id)?;
            step_into(resolved, key, types)
        }
        _ => None,
    }
}

/// Resolve a value that might be a type id reference.
fn resolve_ref(value: &Value, types: &HashMap<String, Value>) -> Value {
    match value {
        Value::String(type_id) => types.get(type_id).cloned().unwrap_or(Value::Null),
        _ => value.clone(),
    }
}

/// Extract keys from a value (object keys, or resolve type ref first).
fn keys_from_value(value: &Value, types: &HashMap<String, Value>) -> Vec<String> {
    match value {
        Value::Object(map) => {
            // Check for dictionary-only case
            if map.len() == 1 && map.contains_key("*") {
                return vec![]; // Dictionary: no spec keys, field names only
            }
            map.keys()
                .filter(|k| *k != "*")
                .cloned()
                .collect()
        }
        Value::String(type_id) => {
            // Resolve and recurse
            if let Some(resolved) = types.get(type_id) {
                keys_from_value(resolved, types)
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

/// Parse JSON text up to cursor position and return the path (key sequence).
/// Arrays are transparent (not included in path).
/// Also returns the prefix being typed (characters after the last opening quote).
pub fn json_path_at_cursor(text: &str) -> (Vec<String>, String) {
    let mut path: Vec<String> = vec![];
    let mut stack: Vec<StackFrame> = vec![]; // Track object/array nesting
    let mut i = 0;
    let bytes = text.as_bytes();
    let len = bytes.len();

    #[derive(Clone)]
    enum StackFrame {
        Object { last_key: Option<String> },
        Array,
    }

    while i < len {
        let c = bytes[i] as char;

        match c {
            '"' => {
                // Check for triple-quote
                if i + 2 < len && bytes[i + 1] == b'"' && bytes[i + 2] == b'"' {
                    // Triple-quoted string: find closing """
                    i += 3;
                    while i + 2 < len {
                        if bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"' {
                            i += 3;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                // Regular string: find closing quote, handling escapes
                let start = i + 1;
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 2; // Skip escaped char
                        continue;
                    }
                    if bytes[i] == b'"' {
                        let s: String = parse_json_string(&text[start..i]);
                        // Update last_key if in object
                        if let Some(StackFrame::Object { last_key }) = stack.last_mut() {
                            *last_key = Some(s);
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                // If we reached the end without closing quote, we're typing a string
                if i >= len {
                    let s = parse_json_string(&text[start..]);
                    if let Some(StackFrame::Object { last_key }) = stack.last_mut() {
                        *last_key = Some(s.clone());
                    }
                    return (path, s);
                }
            }
            '{' => {
                // Push current key to path if in object
                if let Some(StackFrame::Object { last_key }) = stack.last()
                    && let Some(k) = last_key
                {
                    path.push(k.clone());
                }
                stack.push(StackFrame::Object { last_key: None });
                i += 1;
            }
            '}' => {
                if let Some(StackFrame::Object { .. }) = stack.last() {
                    stack.pop();
                    if !path.is_empty() {
                        path.pop();
                    }
                }
                i += 1;
            }
            '[' => {
                // Push current key to path if in object, then push array frame
                if let Some(StackFrame::Object { last_key }) = stack.last()
                    && let Some(k) = last_key
                {
                    path.push(k.clone());
                }
                stack.push(StackFrame::Array);
                i += 1;
            }
            ']' => {
                if let Some(StackFrame::Array) = stack.last() {
                    stack.pop();
                    // Pop the key that led to this array
                    if !path.is_empty() {
                        path.pop();
                    }
                }
                i += 1;
            }
            ':' | ',' | ' ' | '\n' | '\r' | '\t' => {
                i += 1;
            }
            _ => {
                // Skip other content (numbers, true, false, null)
                i += 1;
            }
        }
    }

    (path, String::new())
}

/// Parse a JSON string (handle escape sequences).
fn parse_json_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&escaped) = chars.peek() {
                chars.next();
                match escaped {
                    '"' | '\\' | '/' => result.push(escaped),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    'u' => {
                        // Unicode escape: \uXXXX
                        let hex: String = chars.by_ref().take(4).collect();
                        if let Ok(code) = u32::from_str_radix(&hex, 16)
                            && let Some(ch) = char::from_u32(code)
                        {
                            result.push(ch);
                        }
                    }
                    _ => {
                        result.push('\\');
                        result.push(escaped);
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- keys_at tests ---

    #[test]
    fn search_root_keys_include_query_size_aggs_sort() {
        let keys = keys_at("POST /_search", &[]);
        assert!(keys.contains(&"query".to_string()), "root should have query: {:?}", keys);
        assert!(keys.contains(&"size".to_string()), "root should have size: {:?}", keys);
        assert!(keys.contains(&"aggs".to_string()) || keys.contains(&"aggregations".to_string()),
            "root should have aggs or aggregations: {:?}", keys);
        assert!(keys.contains(&"sort".to_string()), "root should have sort: {:?}", keys);
    }

    #[test]
    fn query_path_includes_bool_match_term_range() {
        let keys = keys_at("POST /_search", &["query".to_string()]);
        assert!(keys.contains(&"bool".to_string()), "query should have bool: {:?}", keys);
        assert!(keys.contains(&"match".to_string()), "query should have match: {:?}", keys);
        assert!(keys.contains(&"term".to_string()), "query should have term: {:?}", keys);
        assert!(keys.contains(&"range".to_string()), "query should have range: {:?}", keys);
    }

    #[test]
    fn query_bool_includes_must_filter_should_must_not() {
        let keys = keys_at("POST /_search", &["query".to_string(), "bool".to_string()]);
        assert!(keys.contains(&"must".to_string()), "bool should have must: {:?}", keys);
        assert!(keys.contains(&"filter".to_string()), "bool should have filter: {:?}", keys);
        assert!(keys.contains(&"should".to_string()), "bool should have should: {:?}", keys);
        assert!(keys.contains(&"must_not".to_string()), "bool should have must_not: {:?}", keys);
    }

    #[test]
    fn query_bool_must_array_includes_match() {
        // Inside bool.must (array), element type is QueryContainer
        let keys = keys_at("POST /_search", &["query".to_string(), "bool".to_string(), "must".to_string()]);
        assert!(keys.contains(&"match".to_string()), "must element should have match: {:?}", keys);
    }

    #[test]
    fn query_match_is_dictionary_no_spec_keys() {
        // match is dictionary_of: field -> MatchQuery
        let keys = keys_at("POST /_search", &["query".to_string(), "match".to_string()]);
        assert!(keys.is_empty(), "match should be dictionary (no spec keys): {:?}", keys);
    }

    #[test]
    fn unknown_endpoint_returns_empty() {
        let keys = keys_at("POST /unknown", &[]);
        assert!(keys.is_empty(), "unknown endpoint should return empty");
    }

    // --- json_path_at_cursor tests ---

    #[test]
    fn path_simple_object() {
        let (path, prefix) = json_path_at_cursor(r#"{"query":{"bo"#);
        assert_eq!(path, vec!["query"], "path should be [query]");
        assert_eq!(prefix, "bo", "prefix should be 'bo'");
    }

    #[test]
    fn path_nested_bool() {
        let (path, prefix) = json_path_at_cursor(r#"{"query":{"bool":{"mu"#);
        assert_eq!(path, vec!["query", "bool"], "path should be [query, bool]");
        assert_eq!(prefix, "mu", "prefix should be 'mu'");
    }

    #[test]
    fn path_through_array() {
        // Arrays are transparent
        let (path, prefix) = json_path_at_cursor(r#"{"query":{"bool":{"must":[{"ma"#);
        assert_eq!(path, vec!["query", "bool", "must"], "path should be [query, bool, must]");
        assert_eq!(prefix, "ma", "prefix should be 'ma'");
    }

    #[test]
    fn path_string_with_brace() {
        // String containing { should not affect path
        let (path, prefix) = json_path_at_cursor(r#"{"query":"value{","ti"#);
        assert_eq!(path, Vec::<String>::new(), "path should be [] (just root after string value)");
        assert_eq!(prefix, "ti", "prefix should be 'ti'");
    }

    #[test]
    fn path_string_with_comma() {
        // String containing , should not affect path
        let (path, prefix) = json_path_at_cursor(r#"{"query":"a,b","ti"#);
        assert_eq!(path, Vec::<String>::new(), "path should be []");
        assert_eq!(prefix, "ti", "prefix should be 'ti'");
    }

    #[test]
    fn path_escaped_quote_in_string() {
        // String with escaped quote
        let (path, prefix) = json_path_at_cursor(r#"{"query":"say \"hello\"","ti"#);
        assert_eq!(path, Vec::<String>::new(), "path should be []");
        assert_eq!(prefix, "ti", "prefix should be 'ti'");
    }

    #[test]
    fn path_triple_quoted_string() {
        // Triple-quoted strings should be skipped properly
        let (path, prefix) = json_path_at_cursor(r#"{"query":"""multi
line""","ti"#);
        assert_eq!(path, Vec::<String>::new(), "path should be []");
        assert_eq!(prefix, "ti", "prefix should be 'ti'");
    }

    #[test]
    fn path_closed_object_returns_to_parent() {
        // After closing brace, we're back at parent level
        let (path, prefix) = json_path_at_cursor(r#"{"query":{},"ti"#);
        assert_eq!(path, Vec::<String>::new(), "path should be []");
        assert_eq!(prefix, "ti", "prefix should be 'ti'");
    }

    #[test]
    fn path_empty_input() {
        let (path, prefix) = json_path_at_cursor("");
        assert_eq!(path, Vec::<String>::new(), "empty input should give empty path");
        assert_eq!(prefix, "", "empty input should give empty prefix");
    }
}
