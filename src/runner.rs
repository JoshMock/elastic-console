use crate::http::{execute, HttpError};
use crate::output::print_response;
use crate::parse::{parse, ParseError};
use crate::Connection;
use std::io::Write;

/// Run all requests from input, writing output to writer.
/// Returns Ok(()) if all requests succeeded (parse + network).
/// HTTP 4xx/5xx are considered successful (the request completed).
pub fn run_all<W: Write>(input: &str, conn: &Connection, out: &mut W) -> Result<(), RunError> {
    let requests = parse(input)?;

    for req in &requests {
        let resp = execute(conn, req)?;
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
    use std::io::{Read, Write as IoWrite};
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
                        let mut buf = [0u8; 4096];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]).to_string();
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
}
