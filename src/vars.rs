use std::collections::HashMap;

/// Substitute `${var}` placeholders in request text with values from vars.
/// Order: (1) `"""${name}"""` → JSON string literal, (2) `"${name}"` → raw if
/// JSON bool/number/array/object else quoted string, (3) any `${name}` → raw.
/// Undefined names are left as-is.
pub fn substitute(text: &str, vars: &HashMap<String, String>) -> String {
    if vars.is_empty() {
        return text.to_string();
    }
    let mut result = text.to_string();

    // Pass 1: """${name}""" → JSON string of value
    result = replace_triple_quoted(&result, vars);

    // Pass 2: "${name}" (whole quoted string) → raw if JSON primitive/array/object, else quoted
    result = replace_double_quoted(&result, vars);

    // Pass 3: remaining ${name} → raw value
    // ponytail: values with quotes inside an embedded string aren't escaped
    result = replace_bare(&result, vars);

    result
}

/// Replace `"""${name}"""` with a JSON string literal of the value.
fn replace_triple_quoted(text: &str, vars: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Look for """${name}"""
        if bytes[i..].starts_with(b"\"\"\"${") 
            && let Some((name, _)) = parse_var_name(&bytes[i + 5..])
        {
            let name_end = i + 5 + name.len();
            if bytes[name_end..].starts_with(b"}\"\"\"") && let Some(val) = vars.get(name) {
                result.push_str(&serde_json::to_string(val).unwrap());
                i = name_end + 4;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Replace `"${name}"` with raw value if it's JSON bool/number/array/object, else escaped string.
fn replace_double_quoted(text: &str, vars: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Look for "${name}"
        if bytes[i..].starts_with(b"\"${") 
            && let Some((name, _)) = parse_var_name(&bytes[i + 3..])
        {
            let name_end = i + 3 + name.len();
            if bytes[name_end..].starts_with(b"}\"") && let Some(val) = vars.get(name) {
                // If value parses as JSON bool/number/array/object, insert raw
                if is_json_non_string(val) {
                    result.push_str(val);
                } else {
                    result.push_str(&serde_json::to_string(val).unwrap());
                }
                i = name_end + 2;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Replace bare ${name} with raw value.
fn replace_bare(text: &str, vars: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i..].starts_with(b"${") 
            && let Some((name, _)) = parse_var_name(&bytes[i + 2..])
        {
            let name_end = i + 2 + name.len();
            if name_end < bytes.len() 
                && bytes[name_end] == b'}' 
                && let Some(val) = vars.get(name) 
            {
                result.push_str(val);
                i = name_end + 1;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Parse a variable name [A-Za-z0-9_]+ from the start of bytes.
/// Returns (name, bytes_consumed) if valid.
fn parse_var_name(bytes: &[u8]) -> Option<(&str, usize)> {
    let mut len = 0;
    while len < bytes.len() {
        let b = bytes[len];
        if b.is_ascii_alphanumeric() || b == b'_' {
            len += 1;
        } else {
            break;
        }
    }
    if len == 0 {
        return None;
    }
    // SAFETY: we only accepted ASCII alphanumeric and underscore
    let name = std::str::from_utf8(&bytes[..len]).ok()?;
    Some((name, len))
}

/// Check if value is valid JSON bool, number, array, or object (not string).
fn is_json_non_string(val: &str) -> bool {
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Quick checks for structural types
    let first = trimmed.as_bytes()[0];
    match first {
        b't' => trimmed == "true",
        b'f' => trimmed == "false",
        b'n' => trimmed == "null",
        b'[' | b'{' => serde_json::from_str::<serde_json::Value>(trimmed).is_ok(),
        b'-' | b'0'..=b'9' => serde_json::from_str::<serde_json::Number>(trimmed).is_ok(),
        _ => false,
    }
}

/// Parse a `:set name value` command. Returns Some((name, value)) if valid.
/// Value is the rest of the line after name, trimmed at end.
pub fn parse_set_command(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with(":set ") {
        return None;
    }
    let rest = trimmed[5..].trim_start();
    let mut chars = rest.chars().peekable();
    let mut name = String::new();
    
    // Parse name [A-Za-z0-9_]+
    while let Some(&c) = chars.peek() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    
    if name.is_empty() {
        return None;
    }
    
    // Skip one space after name, rest is value
    if chars.peek() == Some(&' ') {
        chars.next();
    }
    let value: String = chars.collect();
    Some((name, value.trim_end().to_string()))
}

/// Check if line is a `:vars` command.
pub fn is_vars_command(line: &str) -> bool {
    line.trim() == ":vars"
}

/// Check if line starts with `:` (is a command line).
pub fn is_command(line: &str) -> bool {
    line.trim_start().starts_with(':')
}

/// Format variables for display, sorted by name.
pub fn format_vars(vars: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = vars.iter().collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs.iter().map(|(k, v)| format!("{} = {}", k, v)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn path_substitution() {
        let v = vars(&[("idx", "logs")]);
        assert_eq!(substitute("GET /${idx}/_search", &v), "GET /logs/_search");
    }

    #[test]
    fn quoted_number_becomes_raw() {
        let v = vars(&[("n", "42")]);
        assert_eq!(substitute(r#"{"count":"${n}"}"#, &v), r#"{"count":42}"#);
    }

    #[test]
    fn quoted_array_becomes_raw() {
        let v = vars(&[("n", "[1,2]")]);
        assert_eq!(substitute(r#"{"arr":"${n}"}"#, &v), r#"{"arr":[1,2]}"#);
    }

    #[test]
    fn quoted_bool_becomes_raw() {
        let v = vars(&[("n", "true")]);
        assert_eq!(substitute(r#"{"flag":"${n}"}"#, &v), r#"{"flag":true}"#);
    }

    #[test]
    fn quoted_string_stays_quoted() {
        let v = vars(&[("n", "abc")]);
        assert_eq!(substitute(r#"{"s":"${n}"}"#, &v), r#"{"s":"abc"}"#);
    }

    #[test]
    fn quoted_empty_becomes_empty_string() {
        let v = vars(&[("n", "")]);
        assert_eq!(substitute(r#"{"s":"${n}"}"#, &v), r#"{"s":""}"#);
    }

    #[test]
    fn triple_quoted_number_becomes_string() {
        let v = vars(&[("s", "42")]);
        assert_eq!(substitute(r#"{"x":"""${s}"""}"#, &v), r#"{"x":"42"}"#);
    }

    #[test]
    fn embedded_in_string() {
        let v = vars(&[("who", "world")]);
        assert_eq!(substitute(r#"{"msg":"hello ${who}!"}"#, &v), r#"{"msg":"hello world!"}"#);
    }

    #[test]
    fn undefined_left_as_is_path() {
        let v = HashMap::new();
        assert_eq!(substitute("GET /${idx}/_search", &v), "GET /${idx}/_search");
    }

    #[test]
    fn undefined_left_as_is_quoted() {
        let v = HashMap::new();
        assert_eq!(substitute(r#"{"s":"${n}"}"#, &v), r#"{"s":"${n}"}"#);
    }

    #[test]
    fn undefined_left_as_is_triple() {
        let v = HashMap::new();
        assert_eq!(substitute(r#"{"s":"""${n}"""}"#, &v), r#"{"s":"""${n}"""}"#);
    }

    #[test]
    fn parse_set_with_value() {
        assert_eq!(
            parse_set_command(":set idx logs"),
            Some(("idx".to_string(), "logs".to_string()))
        );
    }

    #[test]
    fn parse_set_empty_value() {
        assert_eq!(
            parse_set_command(":set n "),
            Some(("n".to_string(), "".to_string()))
        );
    }

    #[test]
    fn parse_set_no_value() {
        assert_eq!(
            parse_set_command(":set n"),
            Some(("n".to_string(), "".to_string()))
        );
    }

    #[test]
    fn parse_set_redefine_wins() {
        // Simulate two :set commands for the same name
        let mut v = HashMap::new();
        if let Some((name, val)) = parse_set_command(":set x first") {
            v.insert(name, val);
        }
        if let Some((name, val)) = parse_set_command(":set x second") {
            v.insert(name, val);
        }
        assert_eq!(v.get("x"), Some(&"second".to_string()));
    }

    #[test]
    fn vars_output_sorted() {
        let v = vars(&[("zeta", "z"), ("alpha", "a")]);
        assert_eq!(format_vars(&v), "alpha = a\nzeta = z");
    }

    #[test]
    fn is_command_detects_colon_prefix() {
        assert!(is_command(":set x 1"));
        assert!(is_command(":vars"));
        assert!(is_command("  :unknown"));
        assert!(!is_command("GET /"));
    }
}
