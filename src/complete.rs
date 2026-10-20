use crate::index_source::IndexSource;
use crate::Connection;
use reedline::{CompletionResult, Completer, Span, Suggestion};

const METHODS: &[&str] = &["DELETE", "GET", "HEAD", "POST", "PUT"];

// ponytail: type alias avoids clippy::type_complexity
type BoxedIndexSource = IndexSource<Box<dyn Fn() -> Vec<String> + Send>>;

pub struct EndpointCompleter {
    endpoints: Vec<(String, String)>, // (method, path_template)
    index_source: Option<BoxedIndexSource>,
}

impl EndpointCompleter {
    /// Static-only completion (no live cluster connection).
    #[allow(dead_code)] // ponytail: valid API entry, used in tests
    pub fn new() -> Self {
        Self::with_endpoints_and_source(Self::load_endpoints(), None)
    }

    /// Create with a live cluster connection for index completion.
    pub fn with_connection(conn: Connection) -> Self {
        Self::with_endpoints_and_source(Self::load_endpoints(), Some(IndexSource::new(conn)))
    }

    /// Create with injected index source (for testing).
    #[cfg(test)]
    pub fn with_index_source(source: BoxedIndexSource) -> Self {
        Self::with_endpoints_and_source(Self::load_endpoints(), Some(source))
    }

    fn with_endpoints_and_source(
        endpoints: Vec<(String, String)>,
        index_source: Option<BoxedIndexSource>,
    ) -> Self {
        Self { endpoints, index_source }
    }

    fn load_endpoints() -> Vec<(String, String)> {
        let data = include_str!("../data/endpoints.txt");
        let mut endpoints = Vec::new();
        for line in data.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some((method, path)) = line.split_once(' ') {
                endpoints.push((method.to_string(), path.to_string()));
            }
        }
        endpoints
    }

    /// Find position of cursor's line start (byte offset)
    fn line_start(buffer: &str, pos: usize) -> usize {
        buffer[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    /// Extract the line containing position
    fn line_at_pos(buffer: &str, pos: usize) -> &str {
        let start = Self::line_start(buffer, pos);
        let end = buffer[pos..].find('\n').map(|i| pos + i).unwrap_or(buffer.len());
        &buffer[start..end]
    }

    /// Check if we're inside a JSON body (heuristic: line starts with whitespace or { or [)
    fn is_in_body(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.is_empty()
            || trimmed.starts_with('{')
            || trimmed.starts_with('}')
            || trimmed.starts_with('[')
            || trimmed.starts_with(']')
            || trimmed.starts_with('"')
            || trimmed.starts_with(':')
            || trimmed.starts_with(',')
            || (line.starts_with(char::is_whitespace) && !trimmed.starts_with(|c: char| c.is_ascii_uppercase()))
    }

    /// Complete methods by prefix (case-insensitive)
    fn complete_method(&self, prefix: &str, span: Span) -> Vec<Suggestion> {
        let upper = prefix.to_ascii_uppercase();
        METHODS
            .iter()
            .filter(|m| m.starts_with(&upper))
            .map(|m| Suggestion {
                value: m.to_string(),
                display_override: None,
                description: None,
                style: None,
                extra: None,
                span,
                append_whitespace: true,
                match_indices: None,
            })
            .collect()
    }

    /// Match typed path against template, segment-wise.
    /// Template segments like {param} match any non-empty typed segment.
    /// The last typed segment is prefix-matched.
    /// Returns Some(substituted_path) on match.
    fn match_path(&self, typed: &str, template: &str) -> Option<String> {
        let typed_segs: Vec<&str> = typed.split('/').collect();
        let tmpl_segs: Vec<&str> = template.split('/').collect();

        if typed_segs.is_empty() {
            return None;
        }

        // Must have same or more template segments
        if tmpl_segs.len() < typed_segs.len() {
            return None;
        }

        let mut result_segs = Vec::new();

        for (i, typed_seg) in typed_segs.iter().enumerate() {
            let tmpl_seg = tmpl_segs[i];
            let is_last = i == typed_segs.len() - 1;
            let is_param = tmpl_seg.starts_with('{') && tmpl_seg.ends_with('}');

            if is_param {
                // Param segments match any non-empty typed segment (or empty if last for prefix)
                if is_last {
                    // Last segment: keep typed value (could be empty prefix)
                    result_segs.push(typed_seg.to_string());
                } else if typed_seg.is_empty() {
                    return None; // Non-last param must have a value
                } else {
                    result_segs.push(typed_seg.to_string());
                }
            } else if is_last {
                // Last segment: prefix match
                if !tmpl_seg.starts_with(typed_seg) {
                    return None;
                }
                result_segs.push(tmpl_seg.to_string());
            } else {
                // Non-last: exact match required
                if *typed_seg != tmpl_seg {
                    return None;
                }
                result_segs.push(tmpl_seg.to_string());
            }
        }

        // Append remaining template segments
        for tmpl_seg in &tmpl_segs[typed_segs.len()..] {
            result_segs.push(tmpl_seg.to_string());
        }

        Some(result_segs.join("/"))
    }

    fn complete_path(&mut self, method: &str, typed_path: &str, span: Span) -> Vec<Suggestion> {
        let path = if typed_path.starts_with('/') {
            typed_path.to_string()
        } else {
            format!("/{}", typed_path)
        };

        let mut seen = std::collections::HashSet::new();
        let mut suggestions = Vec::new();

        // Static template suggestions first
        for (m, template) in &self.endpoints {
            if m != method {
                continue;
            }
            if let Some(matched) = self.match_path(&path, template) {
                // Preserve user's leading slash preference
                let value = if typed_path.starts_with('/') {
                    matched
                } else {
                    matched.strip_prefix('/').unwrap_or(&matched).to_string()
                };
                if seen.insert(value.clone()) {
                    suggestions.push(Suggestion {
                        value,
                        display_override: None,
                        description: None,
                        style: None,
                        extra: None,
                        span,
                        append_whitespace: false,
                        match_indices: None,
                    });
                }
            }
        }

        // Live index names when in an {index} slot
        if let Some(live_suggestions) = self.live_index_suggestions(&path, typed_path, span) {
            for s in live_suggestions {
                if seen.insert(s.value.clone()) {
                    suggestions.push(s);
                }
            }
        }

        suggestions
    }

    /// Check if cursor is in an {index} slot and return matching live index names.
    fn live_index_suggestions(&mut self, path: &str, typed_path: &str, span: Span) -> Option<Vec<Suggestion>> {
        let index_source = self.index_source.as_mut()?;

        let typed_segs: Vec<&str> = path.split('/').collect();
        // typed_segs[0] is empty (before leading /), typed_segs[1] is first real segment
        if typed_segs.len() < 2 {
            return None;
        }

        let cursor_seg_idx = typed_segs.len() - 1; // Index of segment being completed
        let prefix = typed_segs[cursor_seg_idx];

        // Check if any matching template has {index} at this segment position
        let has_index_slot = self.endpoints.iter().any(|(_, template)| {
            let tmpl_segs: Vec<&str> = template.split('/').collect();
            if cursor_seg_idx >= tmpl_segs.len() {
                return false;
            }
            // Only offer index completion if this segment is {index}
            // and all prior segments match (either literally or as params)
            if tmpl_segs[cursor_seg_idx] != "{index}" {
                return false;
            }
            // Verify prior segments match
            for i in 1..cursor_seg_idx {
                let typed = typed_segs.get(i).unwrap_or(&"");
                let tmpl = tmpl_segs.get(i).unwrap_or(&"");
                let is_param = tmpl.starts_with('{') && tmpl.ends_with('}');
                if !is_param && *typed != *tmpl {
                    return false;
                }
            }
            true
        });

        if !has_index_slot {
            return None;
        }

        // Get live index names matching prefix
        let names = index_source.names_with_prefix(prefix);
        let has_slash = typed_path.starts_with('/');

        Some(names.into_iter().map(|name| {
            let value = if has_slash {
                format!("/{}", name)
            } else {
                name
            };
            Suggestion {
                value,
                display_override: None,
                description: None,
                style: None,
                extra: None,
                span,
                append_whitespace: false,
                match_indices: None,
            }
        }).collect())
    }
}

impl Completer for EndpointCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let current_line = Self::line_at_pos(line, pos);
        let line_start = Self::line_start(line, pos);
        let pos_in_line = pos - line_start;

        // Skip if in body
        if Self::is_in_body(current_line) {
            return CompletionResult::fresh(vec![]);
        }

        // Skip if querystring present before cursor
        let before_cursor = &current_line[..pos_in_line.min(current_line.len())];
        if before_cursor.contains('?') {
            return CompletionResult::fresh(vec![]);
        }

        // Parse: METHOD PATH (simple whitespace split)
        let mut parts = current_line.split_whitespace();
        let first_word = parts.next().unwrap_or("");
        let second_word = parts.next();

        // Find where first word starts in current_line
        let first_start = current_line.find(first_word).unwrap_or(0);
        let first_end = first_start + first_word.len();

        // Where is cursor?
        if pos_in_line <= first_end {
            // Cursor in or at end of first word (method)
            let prefix = &current_line[first_start..pos_in_line.min(first_end)];
            let span = Span::new(line_start + first_start, line_start + pos_in_line.min(first_end));
            return CompletionResult::fresh(self.complete_method(prefix, span));
        }

        // Cursor past first word - check for path
        if let Some(path_word) = second_word {
            let path_start = current_line[first_end..].find(path_word).map(|i| first_end + i).unwrap_or(first_end + 1);
            let path_end = path_start + path_word.len();

            if pos_in_line >= path_start && pos_in_line <= path_end {
                // Cursor in path word
                let typed_len = pos_in_line - path_start;
                let typed_path = &path_word[..typed_len.min(path_word.len())];
                let span = Span::new(line_start + path_start, line_start + pos_in_line.min(path_end));
                return CompletionResult::fresh(self.complete_path(first_word, typed_path, span));
            }
        } else if pos_in_line > first_end {
            // After method, no path yet - complete empty path
            // Find where the path would start (skip whitespace after method)
            let after_method = &current_line[first_end..];
            let ws_len = after_method.len() - after_method.trim_start().len();
            let path_start = first_end + ws_len;
            
            if pos_in_line >= path_start {
                let span = Span::new(line_start + path_start, line_start + pos_in_line);
                let typed = &current_line[path_start..pos_in_line.min(current_line.len())];
                return CompletionResult::fresh(self.complete_path(first_word, typed, span));
            }
        }

        CompletionResult::fresh(vec![])
    }
}

/// Extract suggestions from CompletionResult for testing
#[cfg(test)]
fn suggestions_from_result(result: CompletionResult) -> Vec<Suggestion> {
    match result {
        CompletionResult::Fresh { suggestions, .. } => suggestions.iter().cloned().collect(),
        CompletionResult::Stale { suggestions, .. } => suggestions.iter().cloned().collect(),
        CompletionResult::Pending => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(line: &str, pos: usize) -> Vec<String> {
        let mut c = EndpointCompleter::new();
        suggestions_from_result(c.complete(line, pos))
            .into_iter()
            .map(|s| s.value)
            .collect()
    }

    fn complete_with_spans(line: &str, pos: usize) -> Vec<(String, usize, usize)> {
        let mut c = EndpointCompleter::new();
        suggestions_from_result(c.complete(line, pos))
            .into_iter()
            .map(|s| (s.value, s.span.start, s.span.end))
            .collect()
    }

    #[test]
    fn method_prefix_g_returns_get() {
        let results = complete("g", 1);
        assert!(results.contains(&"GET".to_string()), "expected GET in {:?}", results);
    }

    #[test]
    fn method_prefix_case_insensitive() {
        let results = complete("de", 2);
        assert!(results.contains(&"DELETE".to_string()), "expected DELETE in {:?}", results);
    }

    #[test]
    fn path_cat_indices() {
        let results = complete("GET /_cat/ind", 13);
        assert!(
            results.iter().any(|s| s == "/_cat/indices"),
            "expected /_cat/indices in {:?}",
            results
        );
    }

    #[test]
    fn path_param_substitution() {
        // Typing `/my-idx/_se` with GET should match `/{index}/_search` → `/my-idx/_search`
        let results = complete("GET /my-idx/_se", 15);
        assert!(
            results.iter().any(|s| s == "/my-idx/_search"),
            "expected /my-idx/_search in {:?}",
            results
        );
    }

    #[test]
    fn method_filter_put_only_not_for_get() {
        // /_bulk is POST/PUT only, not GET
        let get_results = complete("GET /_bul", 9);
        assert!(
            !get_results.iter().any(|s| s == "/_bulk"),
            "GET should not offer /_bulk, got {:?}",
            get_results
        );

        let put_results = complete("PUT /_bul", 9);
        assert!(
            put_results.iter().any(|s| s == "/_bulk"),
            "PUT should offer /_bulk, got {:?}",
            put_results
        );
    }

    #[test]
    fn span_covers_word_being_replaced() {
        let results = complete_with_spans("GET /_cat/ind", 13);
        // Path starts at position 4, cursor at 13
        assert!(!results.is_empty(), "should have suggestions");
        let (_, start, end) = &results[0];
        assert_eq!(*start, 4, "span should start at path position");
        assert_eq!(*end, 13, "span should end at cursor");
    }

    #[test]
    fn querystring_no_suggestions() {
        let results = complete("GET /_search?q=", 15);
        assert!(results.is_empty(), "querystring should return no suggestions");
    }

    #[test]
    fn body_no_suggestions() {
        let results = complete("{\"query\":", 9);
        assert!(results.is_empty(), "body line should return no suggestions");
    }

    #[test]
    fn multiline_second_request() {
        let buffer = "GET /_search\n{}\nGET /_cat/ind";
        // Cursor at position: 16 (start of second GET) + 13 = 29 is end of `ind`
        // "GET /_search\n" = 13 chars, "{}\n" = 3 chars, so second request starts at 16
        // "GET /_cat/ind" cursor at end = position 16 + 13 = 29
        let pos = 16 + 13; // = 29
        let results = complete(buffer, pos);
        assert!(
            results.iter().any(|s| s == "/_cat/indices"),
            "multiline: expected /_cat/indices in {:?}",
            results
        );
    }

    #[test]
    fn path_without_leading_slash() {
        // User types `_cat/ind` without leading slash
        let results = complete("GET _cat/ind", 12);
        assert!(
            results.iter().any(|s| s == "_cat/indices"),
            "should accept path without leading slash, got {:?}",
            results
        );
    }

    #[test]
    fn empty_path_suggests_all_for_method() {
        let results = complete("GET /", 5);
        // Should have many suggestions
        assert!(results.len() > 10, "should have many path suggestions, got {}", results.len());
    }

    // --- Live index completion tests ---

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;


    fn completer_with_live_indexes(indexes: Vec<String>) -> EndpointCompleter {
        use crate::index_source::IndexSource;
        let source = IndexSource::new_from_boxed(Box::new(move || indexes.clone()));
        EndpointCompleter::with_index_source(source)
    }

    #[test]
    fn live_index_in_index_slot() {
        // GET /my + Tab should offer live indexes starting with "my"
        let mut c = completer_with_live_indexes(vec!["my-logs".into(), "my-metrics".into(), "other".into()]);
        let results: Vec<String> = suggestions_from_result(c.complete("GET /my", 7))
            .into_iter().map(|s| s.value).collect();

        assert!(results.contains(&"/my-logs".to_string()), "expected /my-logs in {:?}", results);
        assert!(results.contains(&"/my-metrics".to_string()), "expected /my-metrics in {:?}", results);
        assert!(!results.contains(&"/other".to_string()), "should not include non-matching /other");
    }

    #[test]
    fn static_templates_still_work_with_live_source() {
        // Static templates like /_cat should still complete
        let mut c = completer_with_live_indexes(vec!["my-logs".into()]);
        let results: Vec<String> = suggestions_from_result(c.complete("GET /_ca", 8))
            .into_iter().map(|s| s.value).collect();

        assert!(results.iter().any(|s| s.starts_with("/_cat")), "expected /_cat in {:?}", results);
    }

    #[test]
    fn static_templates_before_live_indexes() {
        // When both match, static templates should come first
        let mut c = completer_with_live_indexes(vec!["my-logs".into()]);
        let results: Vec<String> = suggestions_from_result(c.complete("GET /", 5))
            .into_iter().map(|s| s.value).collect();

        // Find first static (starts with /_) and first live
        let first_static = results.iter().position(|s| s.starts_with("/_"));
        let first_live = results.iter().position(|s| s == "/my-logs");

        if let (Some(static_pos), Some(live_pos)) = (first_static, first_live) {
            assert!(static_pos < live_pos, "static templates should come before live indexes");
        }
    }

    #[test]
    fn later_segment_uses_static_completion() {
        // GET /my-logs/_se should complete to /_search from templates, not offer live indexes
        let mut c = completer_with_live_indexes(vec!["my-logs".into()]);
        let results: Vec<String> = suggestions_from_result(c.complete("GET /my-logs/_se", 16))
            .into_iter().map(|s| s.value).collect();

        assert!(results.iter().any(|s| s.contains("_search")), "expected _search in {:?}", results);
    }

    #[test]
    fn no_live_fetch_on_method_completion() {
        // Method completion should not trigger a fetch
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        use crate::index_source::IndexSource;
        let source = IndexSource::new_from_boxed(Box::new(
            move || { cc.fetch_add(1, Ordering::SeqCst); vec![] },
        ));
        let mut c = EndpointCompleter::with_index_source(source);

        c.complete("G", 1); // Method completion
        assert_eq!(call_count.load(Ordering::SeqCst), 0, "should not fetch on method completion");
    }
}
