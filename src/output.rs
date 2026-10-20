use crate::http::Response;
use nu_ansi_term::Color;
use std::io::Write;

pub fn print_response<W: Write>(w: &mut W, resp: &Response) {
    // Status line with color
    let status_color = if resp.status < 300 {
        Color::Green
    } else if resp.status < 400 {
        Color::Yellow
    } else {
        Color::Red
    };

    let _ = writeln!(
        w,
        "{}",
        status_color.paint(format!("{} {}", resp.status, resp.status_text))
    );

    // Body: pretty-print if JSON, else raw
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&resp.body) {
        if let Ok(pretty) = serde_json::to_string_pretty(&json) {
            let _ = writeln!(w, "{}", pretty);
        } else {
            let _ = writeln!(w, "{}", resp.body);
        }
    } else if !resp.body.is_empty() {
        let _ = writeln!(w, "{}", resp.body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pretty_json_output() {
        let resp = Response {
            status: 200,
            status_text: "OK".to_string(),
            body: r#"{"name":"test","version":"8.0"}"#.to_string(),
        };
        let mut buf = Vec::new();
        print_response(&mut buf, &resp);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("200 OK"));
        // Pretty-printed JSON should have newlines
        assert!(output.contains("\"name\": \"test\""));
    }

    #[test]
    fn test_non_json_output() {
        let resp = Response {
            status: 200,
            status_text: "OK".to_string(),
            body: "plain text response".to_string(),
        };
        let mut buf = Vec::new();
        print_response(&mut buf, &resp);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("plain text response"));
    }

    #[test]
    fn test_empty_body() {
        let resp = Response {
            status: 204,
            status_text: "No Content".to_string(),
            body: String::new(),
        };
        let mut buf = Vec::new();
        print_response(&mut buf, &resp);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("204"));
        // Should not have extra blank lines for empty body
        assert_eq!(output.trim().lines().count(), 1);
    }
}
