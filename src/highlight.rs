use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

pub struct SyntaxHighlighter {
    parser: RefCell<Parser>,
    query: Query,
    /// Shared cursor position, updated on every highlight() call.
    /// ponytail: approach (b) from PLAN - highlighter gets cursor on final repaint before Submit returns.
    cursor_pos: Arc<AtomicUsize>,
}

impl SyntaxHighlighter {
    #[allow(dead_code)] // Used in tests
    pub fn new() -> Self {
        Self::with_cursor_tracker(Arc::new(AtomicUsize::new(0)))
    }

    /// Create a highlighter that shares cursor position with the caller.
    pub fn with_cursor_tracker(cursor_pos: Arc<AtomicUsize>) -> Self {
        let language = tree_sitter_elasticsearch_console::LANGUAGE;
        let mut parser = Parser::new();
        parser
            .set_language(&language.into())
            .expect("grammar version mismatch");
        let query = Query::new(
            &language.into(),
            tree_sitter_elasticsearch_console::HIGHLIGHTS_QUERY,
        )
        .expect("invalid highlights query");

        Self {
            parser: RefCell::new(parser),
            query,
            cursor_pos,
        }
    }

    fn style_for_capture(&self, capture_name: &str) -> Style {
        match capture_name {
            "keyword" => Color::Cyan.bold(),
            "function" => Color::Yellow.normal(),
            "string" => Color::Green.normal(),
            "number" => Color::Purple.normal(),
            "boolean" => Color::Purple.normal(),
            "constant" => Color::Purple.normal(),
            "comment" => Color::DarkGray.normal(),
            "punctuation.bracket" | "punctuation.delimiter" | "punctuation" => {
                Color::White.normal()
            }
            _ => Style::default(),
        }
    }
}

impl Highlighter for SyntaxHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> StyledText {
        // Record cursor position for run-under-cursor feature
        self.cursor_pos.store(cursor, Ordering::Relaxed);

        let tree = self.parser.borrow_mut().parse(line, None);

        let Some(tree) = tree else {
            // Parse failed completely, return unstyled
            let mut result = StyledText::new();
            result.push((Style::default(), line.to_string()));
            return result;
        };

        let mut cursor = QueryCursor::new();
        let mut highlights = Vec::new();

        let mut matches = cursor.matches(&self.query, tree.root_node(), line.as_bytes());
        while let Some(qmatch) = matches.next() {
            for capture in qmatch.captures {
                let capture_name = &self.query.capture_names()[capture.index as usize];
                // `@none` spans the whole body; keeping it would mask the JSON captures inside.
                if *capture_name == "none" {
                    continue;
                }
                let style = self.style_for_capture(capture_name);
                let range = capture.node.byte_range();
                highlights.push((range.start, range.end, style));
            }
        }

        // Sort by start position, then end position (descending for overlaps)
        highlights.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));

        // Build styled segments, filling gaps with default style
        let bytes = line.as_bytes();
        let mut result = StyledText::new();
        let mut pos = 0;

        for (start, end, style) in highlights {
            if start > pos {
                // Fill gap with unstyled text
                result.push((Style::default(), String::from_utf8_lossy(&bytes[pos..start]).into_owned()));
            }
            if end > pos {
                // Add styled segment, skip if we've already covered this range
                let segment_start = pos.max(start);
                result.push((style, String::from_utf8_lossy(&bytes[segment_start..end]).into_owned()));
                pos = end;
            }
        }

        // Final gap
        if pos < bytes.len() {
            result.push((Style::default(), String::from_utf8_lossy(&bytes[pos..]).into_owned()));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_equals_input_empty() {
        let h = SyntaxHighlighter::new();
        let styled = h.highlight("", 0);
        assert_eq!(styled.raw_string(), "");
    }

    #[test]
    fn test_output_equals_input_partial() {
        let h = SyntaxHighlighter::new();
        let input = "POST /x\n{\"a\":";
        let styled = h.highlight(input, 0);
        assert_eq!(styled.raw_string(), input);
    }

    #[test]
    fn test_output_equals_input_complete() {
        let h = SyntaxHighlighter::new();
        let input = "GET /_search\n{\"query\":{\"match_all\":{}}}";
        let styled = h.highlight(input, 0);
        assert_eq!(styled.raw_string(), input);
    }

    #[test]
    fn test_method_and_path_styled() {
        let h = SyntaxHighlighter::new();
        let styled = h.highlight("GET /_search", 0);
        
        // Verify text is preserved
        assert_eq!(styled.raw_string(), "GET /_search");

        // Check that GET has keyword style (cyan bold)
        let keyword_style = h.style_for_capture("keyword");
        let path_style = h.style_for_capture("function");

        // Access the buffer field directly (it's public in StyledText)
        let buffer = &styled.buffer;
        assert!(buffer.len() >= 2, "expected at least 2 segments, got {}", buffer.len());

        // Find GET segment
        let get_seg = buffer.iter().find(|(_, s)| s == "GET");
        assert!(get_seg.is_some(), "GET segment not found");
        assert_eq!(get_seg.unwrap().0, keyword_style, "GET should have keyword style");

        // Find /_search segment
        let path_seg = buffer.iter().find(|(_, s)| s == "/_search");
        assert!(path_seg.is_some(), "/_search segment not found");
        assert_eq!(path_seg.unwrap().0, path_style, "/_search should have function/path style");
    }

    #[test]
    fn test_json_body_preserves_text() {
        let h = SyntaxHighlighter::new();
        // Test that JSON body text is preserved
        let input = r#"POST /test
{"field": "value", "count": 42}"#;
        let styled = h.highlight(input, 0);

        // The key requirement: all text must be preserved
        assert_eq!(styled.raw_string(), input);

        // Verify method and path are styled
        let buffer = &styled.buffer;
        let keyword_style = h.style_for_capture("keyword");
        let path_style = h.style_for_capture("function");
        
        let has_method = buffer.iter().any(|(style, text)| {
            *style == keyword_style && text == "POST"
        });
        assert!(has_method, "POST should be styled as keyword");
        
        let has_path = buffer.iter().any(|(style, text)| {
            *style == path_style && text == "/test"
        });
        assert!(has_path, "/test should be styled as path");
    }

    #[test]
    fn test_json_body_values_styled() {
        let h = SyntaxHighlighter::new();
        let styled = h.highlight("POST /test\n{\"count\": 42, \"ok\": true}\n", 0);
        let style_of = |t: &str| styled.buffer.iter().find(|(_, s)| s == t).map(|(st, _)| *st);
        assert_eq!(style_of("42"), Some(Color::Purple.normal()));
        assert_eq!(style_of("true"), Some(Color::Purple.normal()));
        assert_eq!(style_of("{"), Some(Color::White.normal()));
    }

    #[test]
    fn test_cursor_tracking() {
        let cursor_pos = Arc::new(AtomicUsize::new(0));
        let h = SyntaxHighlighter::with_cursor_tracker(cursor_pos.clone());

        // Simulate highlight calls as reedline would make them
        h.highlight("GET /\nPOST /_search", 5);  // cursor at position 5
        assert_eq!(cursor_pos.load(Ordering::Relaxed), 5);

        h.highlight("GET /\nPOST /_search", 12); // cursor moved to position 12
        assert_eq!(cursor_pos.load(Ordering::Relaxed), 12);
    }
}
