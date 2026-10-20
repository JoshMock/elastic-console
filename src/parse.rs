use tree_sitter::{Node, Parser, Tree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub bodies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parse console input into requests.
pub fn parse(src: &str) -> Result<Vec<Request>, ParseError> {
    // ponytail: grammar requires trailing newline after request line and bodies
    let normalized = if src.ends_with('\n') {
        src.to_string()
    } else {
        format!("{}\n", src)
    };

    let tree = parse_tree(&normalized)?;
    extract_requests(&tree, &normalized)
}

/// Find the request containing the given byte offset.
/// ponytail: prepared for "run request under cursor" feature
pub fn request_at_offset(src: &str, offset: usize) -> Result<Option<Request>, ParseError> {
    let normalized = if src.ends_with('\n') {
        src.to_string()
    } else {
        format!("{}\n", src)
    };

    let tree = parse_tree(&normalized)?;
    let root = tree.root_node();

    for i in 0..root.named_child_count() {
        if let Some(req_node) = root.named_child(i)
            && req_node.kind() == "request"
            && req_node.start_byte() <= offset
            && offset < req_node.end_byte()
        {
            return extract_single_request(&req_node, &normalized).map(Some);
        }
    }
    Ok(None)
}

/// Select the request to run based on cursor position (Kibana Console behaviour).
/// - If cursor is inside a request: return that request
/// - If cursor is between requests or on a blank line: return the nearest preceding request
/// - If there's no preceding request: return the first one
pub fn select_request_at_cursor(src: &str, cursor: usize) -> Result<Option<Request>, ParseError> {
    // First try exact match
    if let Some(req) = request_at_offset(src, cursor)? {
        return Ok(Some(req));
    }

    // Cursor not inside a request - find nearest preceding or first
    let normalized = if src.ends_with('\n') {
        src.to_string()
    } else {
        format!("{}\n", src)
    };

    let tree = parse_tree(&normalized)?;
    let root = tree.root_node();

    let mut preceding: Option<Node> = None;
    let mut first_req: Option<Node> = None;

    for i in 0..root.named_child_count() {
        if let Some(req_node) = root.named_child(i)
            && req_node.kind() == "request"
        {
            if first_req.is_none() {
                first_req = Some(req_node);
            }
            if req_node.end_byte() <= cursor {
                preceding = Some(req_node);
            }
        }
    }

    // Return preceding if found, else first
    let target = preceding.or(first_req);
    match target {
        Some(node) => extract_single_request(&node, &normalized).map(Some),
        None => Ok(None),
    }
}

fn parse_tree(src: &str) -> Result<Tree, ParseError> {
    let mut parser = Parser::new();
    let lang = tree_sitter_elasticsearch_console::LANGUAGE;
    parser
        .set_language(&lang.into())
        .expect("tree-sitter version mismatch");

    parser.parse(src, None).ok_or_else(|| ParseError {
        line: 1,
        message: "failed to parse".to_string(),
    })
}

fn extract_requests(tree: &Tree, src: &str) -> Result<Vec<Request>, ParseError> {
    let root = tree.root_node();

    // Check for ERROR or MISSING nodes
    if let Some(err) = find_error_node(&root, src) {
        return Err(err);
    }

    let mut requests = Vec::new();
    for i in 0..root.named_child_count() {
        if let Some(req_node) = root.named_child(i)
            && req_node.kind() == "request"
        {
            requests.push(extract_single_request(&req_node, src)?);
        }
    }
    Ok(requests)
}

fn extract_single_request(req_node: &Node, src: &str) -> Result<Request, ParseError> {
    let mut method = None;
    let mut path = None;
    let mut bodies = Vec::new();

    for i in 0..req_node.child_count() {
        if let Some(child) = req_node.child(i) {
            match child.kind() {
                "request_line" => {
                    // Grammar update: method and path are inside request_line
                    for j in 0..child.child_count() {
                        if let Some(rl_child) = child.child(j) {
                            match rl_child.kind() {
                                "method" => method = Some(node_text(&rl_child, src)),
                                "path" => path = Some(node_text(&rl_child, src)),
                                _ => {}
                            }
                        }
                    }
                }
                "body" => {
                    // Grammar update: body may contain named children (json_string) or unnamed tokens
                    // If there are named children, extract each json_string
                    // Otherwise, the body text is in the node's full span
                    let named_count = child.named_child_count();
                    if named_count > 0 {
                        // Multiple json bodies (bulk/msearch)
                        for j in 0..named_count {
                            if let Some(json_node) = child.named_child(j) {
                                let body_text = node_text(&json_node, src).trim().to_string();
                                if !body_text.is_empty() {
                                    bodies.push(body_text);
                                }
                            }
                        }
                    } else {
                        // Single body - extract the full text
                        let body_text = node_text(&child, src).trim().to_string();
                        if !body_text.is_empty() {
                            bodies.push(body_text);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let method = method.ok_or_else(|| ParseError {
        line: req_node.start_position().row + 1,
        message: "missing method".to_string(),
    })?;

    let path = path.ok_or_else(|| ParseError {
        line: req_node.start_position().row + 1,
        message: "missing path".to_string(),
    })?;

    Ok(Request {
        method,
        path,
        bodies,
    })
}

fn find_error_node(node: &Node, src: &str) -> Option<ParseError> {
    if node.is_error() {
        return Some(ParseError {
            line: node.start_position().row + 1,
            message: format!("syntax error near '{}'", truncate(node_text(node, src), 20)),
        });
    }
    if node.is_missing() {
        return Some(ParseError {
            line: node.start_position().row + 1,
            message: format!("missing {}", node.kind()),
        });
    }

    // Recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && let Some(err) = find_error_node(&child, src)
        {
            return Some(err);
        }
    }
    None
}

fn node_text(node: &Node, src: &str) -> String {
    src[node.start_byte()..node.end_byte()].to_string()
}

fn truncate(s: String, max: usize) -> String {
    if s.len() <= max {
        s
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_get() {
        let input = "GET /\n";
        let requests = parse(input).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/");
        assert!(requests[0].bodies.is_empty());
    }

    #[test]
    fn test_get_without_trailing_newline() {
        let input = "GET /";
        let requests = parse(input).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/");
    }

    #[test]
    fn test_post_with_body() {
        let input = r#"POST /_search
{"query": {"match_all": {}}}
"#;
        let requests = parse(input).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/_search");
        assert_eq!(requests[0].bodies.len(), 1);
        assert!(requests[0].bodies[0].contains("match_all"));
    }

    #[test]
    fn test_multiple_requests() {
        let input = "GET /\n\nGET /_cat/indices\n";
        let requests = parse(input).unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/");
        assert_eq!(requests[1].path, "/_cat/indices");
    }

    #[test]
    fn test_bulk_bodies() {
        let input = r#"POST /_bulk
{"index": {"_index": "test"}}
{"field": "value"}
"#;
        let requests = parse(input).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].bodies.len(), 2);
    }

    #[test]
    fn test_parse_error_invalid_method() {
        let input = "INVALID /path\n";
        let result = parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_path_with_query_string() {
        let input = "GET /_cat/indices?v=true&h=index,health\n";
        let requests = parse(input).unwrap();
        assert_eq!(requests[0].path, "/_cat/indices?v=true&h=index,health");
    }

    #[test]
    fn test_request_at_offset() {
        let input = "GET /\n\nPOST /_search\n{}\n";
        // Offset in the POST request
        let result = request_at_offset(input, 10).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().method, "POST");
    }

    // --- select_request_at_cursor tests ---

    #[test]
    fn select_cursor_in_first_of_two() {
        let input = "GET /\n\nPOST /_search\n";
        // Cursor at position 2, inside "GET /"
        let req = select_request_at_cursor(input, 2).unwrap().unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/");
    }

    #[test]
    fn select_cursor_in_second_of_two() {
        let input = "GET /\n\nPOST /_search\n";
        // Cursor at position 10, inside "POST /_search"
        let req = select_request_at_cursor(input, 10).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/_search");
    }

    #[test]
    fn select_cursor_on_blank_between() {
        let input = "GET /\n\nPOST /_search\n";
        // Cursor at position 6 (the blank line between requests)
        let req = select_request_at_cursor(input, 6).unwrap().unwrap();
        // Should return preceding request (GET)
        assert_eq!(req.method, "GET");
    }

    #[test]
    fn select_cursor_at_buffer_end() {
        let input = "GET /\n\nPOST /_search\n";
        let len = input.len();
        let req = select_request_at_cursor(input, len).unwrap().unwrap();
        // Should return the last (preceding) request
        assert_eq!(req.method, "POST");
    }

    #[test]
    fn select_cursor_inside_body() {
        let input = "POST /_search\n{\"query\": {}}\n";
        // Cursor inside the body JSON
        let req = select_request_at_cursor(input, 20).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.bodies.len(), 1);
    }

    #[test]
    fn select_cursor_before_first_request() {
        // Edge case: blank lines before first request
        let input = "\n\nGET /\n";
        // Cursor at position 0, before any request
        let req = select_request_at_cursor(input, 0).unwrap().unwrap();
        // Should return first request
        assert_eq!(req.method, "GET");
    }

    #[test]
    fn select_single_request_unchanged() {
        let input = "GET /_cat/health\n";
        let req = select_request_at_cursor(input, 5).unwrap().unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/_cat/health");
    }
}
