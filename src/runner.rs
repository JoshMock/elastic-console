use crate::http::{execute, HttpError};
use crate::output::print_response;
use crate::parse::{parse, ParseError};
use crate::vars::{format_vars, is_command, is_vars_command, parse_set_command, substitute_request};
use crate::Connection;
use std::collections::HashMap;
use std::io::Write;

/// Run all requests from input, writing output to writer.
/// Returns Ok(()) if all requests succeeded (parse + network).
/// HTTP 4xx/5xx are considered successful (the request completed).
pub fn run_all<W: Write>(input: &str, conn: &Connection, out: &mut W) -> Result<(), RunError> {
    run_all_with_vars(input, conn, out, &mut HashMap::new())
}

/// Run all requests, processing :set/:vars commands and substituting variables.
/// Vars are updated in place so piped scripts can define then use variables.
pub fn run_all_with_vars<W: Write>(
    input: &str,
    conn: &Connection,
    out: &mut W,
    vars: &mut HashMap<String, String>,
) -> Result<(), RunError> {
    // Split into chunks: command lines vs request text
    let mut request_chunk = String::new();

    for line in input.lines() {
        if is_command(line) {
            // Flush any accumulated request text first
            if !request_chunk.trim().is_empty() {
                run_request_chunk(&request_chunk, conn, out, vars)?;
                request_chunk.clear();
            }
            // Process command
            if let Some((name, val)) = parse_set_command(line) {
                vars.insert(name, val);
            } else if is_vars_command(line) {
                writeln!(out, "{}", format_vars(vars)).ok();
            } else {
                // Unknown command
                writeln!(out, "unknown command: {}", line.trim()).ok();
            }
        } else {
            request_chunk.push_str(line);
            request_chunk.push('\n');
        }
    }

    // Flush remaining request text
    if !request_chunk.trim().is_empty() {
        run_request_chunk(&request_chunk, conn, out, vars)?;
    }

    Ok(())
}

fn run_request_chunk<W: Write>(
    chunk: &str,
    conn: &Connection,
    out: &mut W,
    vars: &HashMap<String, String>,
) -> Result<(), RunError> {
    for req in parse(chunk)? {
        let resp = execute(conn, &substitute_request(req, vars))?;
        print_response(out, &resp);
        // ponytail: HTTP 4xx/5xx still means the request completed; not an error
        // Document: piped mode exits 0 on HTTP errors, non-zero only on parse/network errors
    }
    Ok(())
}

#[derive(Debug)]
pub enum RunError {
    Parse(ParseError),
    Http(HttpError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Parse(e) => write!(f, "parse error: {}", e),
            RunError::Http(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for RunError {}

impl From<ParseError> for RunError {
    fn from(e: ParseError) -> Self {
        RunError::Parse(e)
    }
}

impl From<HttpError> for RunError {
    fn from(e: HttpError) -> Self {
        RunError::Http(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Auth;
    use std::io::Write as IoWrite;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    struct MockServer {
        addr: String,
        shutdown_tx: mpsc::Sender<()>,
    }

    impl MockServer {
        fn start<F>(handler: F) -> Self
        where
            F: Fn(&str) -> String + Send + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let (shutdown_tx, shutdown_rx) = mpsc::channel();

            thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }
                    if let Ok((mut stream, _)) = listener.accept() {
                        stream.set_nonblocking(false).unwrap();
                        let req = crate::http::read_test_request(&mut stream);
                        let response = handler(&req);
                        let _ = stream.write_all(response.as_bytes());
                    }
                    thread::sleep(std::time::Duration::from_millis(10));
                }
            });

            MockServer { addr, shutdown_tx }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            let _ = self.shutdown_tx.send(());
        }
    }

    fn ok_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    #[test]
    fn test_run_single_request() {
        let server = MockServer::start(|_| ok_response(r#"{"ok":true}"#));
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let mut out = Vec::new();

        let result = run_all("GET /\n", &conn, &mut out);
        assert!(result.is_ok());

        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("200 OK"));
        assert!(output.contains("\"ok\": true"));
    }

    #[test]
    fn test_run_multiple_requests() {
        let server = MockServer::start(|req| {
            if req.contains("GET / ") {
                ok_response(r#"{"endpoint":"root"}"#)
            } else {
                ok_response(r#"{"endpoint":"other"}"#)
            }
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let mut out = Vec::new();

        let input = "GET /\n\nGET /_cat/health\n";
        let result = run_all(input, &conn, &mut out);
        assert!(result.is_ok());

        let output = String::from_utf8(out).unwrap();
        // Should have two 200 OK lines
        assert_eq!(output.matches("200 OK").count(), 2);
    }

    #[test]
    fn test_run_parse_error() {
        let conn = Connection {
            url: "http://localhost:9200".to_string(),
            auth: Auth::None,
        };
        let mut out = Vec::new();

        let result = run_all("INVALID /path\n", &conn, &mut out);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RunError::Parse(_)));
    }

    #[test]
    fn test_http_4xx_is_not_error() {
        let server = MockServer::start(|_| {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 18\r\n\r\n{\"found\":false}".to_string()
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let mut out = Vec::new();

        let result = run_all("GET /missing\n", &conn, &mut out);
        // HTTP 404 should not be an error - request completed
        assert!(result.is_ok());

        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("404"));
    }

    #[test]
    fn test_piped_set_then_request() {
        use std::sync::{Arc, Mutex};
        let received_path = Arc::new(Mutex::new(String::new()));
        let path_clone = Arc::clone(&received_path);

        let server = MockServer::start(move |req| {
            // Extract path from request line "GET /path HTTP/1.1"
            if let Some(line) = req.lines().next() {
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    *path_clone.lock().unwrap() = parts[1].to_string();
                }
            }
            ok_response(r#"{}"#)
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let mut out = Vec::new();

        let input = ":set idx logs\nGET /${idx}/_search\n";
        let result = run_all(input, &conn, &mut out);
        assert!(result.is_ok());

        // Server should have received /logs/_search, not /${idx}/_search
        assert_eq!(*received_path.lock().unwrap(), "/logs/_search");
    }

    #[test]
    fn test_piped_embedded_value_with_quotes_is_sent_escaped() {
        use std::sync::{Arc, Mutex};
        let received = Arc::new(Mutex::new(String::new()));
        let clone = Arc::clone(&received);
        let server = MockServer::start(move |req| {
            *clone.lock().unwrap() = req.to_string();
            ok_response(r#"{}"#)
        });
        let conn = Connection { url: server.url(), auth: Auth::None };
        let mut out = Vec::new();

        let input = ":set v say \"hi\"\nPOST /x/_search\n{\"q\": \"a ${v} b\"}\n";
        let result = run_all(input, &conn, &mut out);
        assert!(result.is_ok(), "{:?}", result.err());
        let got = received.lock().unwrap().clone();
        assert!(got.contains(r#"{"q": "a say \"hi\" b"}"#), "{}", got);
    }

    #[test]
    fn test_vars_command_output() {
        let conn = Connection {
            url: "http://localhost:9200".to_string(),
            auth: Auth::None,
        };
        let mut out = Vec::new();
        let mut vars = HashMap::new();
        vars.insert("z".to_string(), "last".to_string());
        vars.insert("a".to_string(), "first".to_string());

        // :vars with no requests doesn't need a server
        let input = ":vars\n";
        let result = run_all_with_vars(input, &conn, &mut out, &mut vars);
        assert!(result.is_ok());

        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("a = first"));
        assert!(output.contains("z = last"));
        // Should be sorted, so a comes before z
        let a_pos = output.find("a = first").unwrap();
        let z_pos = output.find("z = last").unwrap();
        assert!(a_pos < z_pos);
    }
}
