use crate::{Auth, Connection, Request};
use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub body: String,
}

#[derive(Debug)]
pub enum HttpError {
    Network(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Network(msg) => write!(f, "network error: {}", msg),
        }
    }
}

impl std::error::Error for HttpError {}

pub fn execute(conn: &Connection, req: &Request) -> Result<Response, HttpError> {
    let url = build_url(&conn.url, &req.path);
    let is_ndjson = is_ndjson_endpoint(&req.path) || req.bodies.len() > 1;

    let method = req.method.to_uppercase();
    // ponytail: http_status_as_error(false) so 4xx/5xx return Ok with body, not Err
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build();
    let agent: ureq::Agent = config.into();

    // Prepare body and content type
    let (body, content_type): (Option<String>, Option<&str>) = if req.bodies.is_empty() {
        (None, None)
    } else if is_ndjson {
        let mut ndjson = req.bodies.join("\n");
        ndjson.push('\n');
        (Some(ndjson), Some("application/x-ndjson"))
    } else {
        (Some(req.bodies[0].clone()), Some("application/json"))
    };

    // Build auth header value
    let auth_header: Option<String> = match &conn.auth {
        Auth::ApiKey(key) => Some(format!("ApiKey {}", key)),
        Auth::Basic { username, password } => {
            let encoded = STANDARD.encode(format!("{}:{}", username, password));
            Some(format!("Basic {}", encoded))
        }
        Auth::None => None,
    };

    // ureq 3 uses specific method calls; match and dispatch
    let response = match method.as_str() {
        "GET" => send_request(agent.get(&url), body, content_type, auth_header.as_deref()),
        "POST" => send_request_with_body(agent.post(&url), body, content_type, auth_header.as_deref()),
        "PUT" => send_request_with_body(agent.put(&url), body, content_type, auth_header.as_deref()),
        "DELETE" => send_request(agent.delete(&url), body, content_type, auth_header.as_deref()),
        "HEAD" => send_request(agent.head(&url), body, content_type, auth_header.as_deref()),
        "PATCH" => send_request_with_body(agent.patch(&url), body, content_type, auth_header.as_deref()),
        "OPTIONS" => send_request(agent.options(&url), body, content_type, auth_header.as_deref()),
        _ => return Err(HttpError::Network(format!("unsupported method: {}", method))),
    };

    match response {
        Ok(mut resp) => {
            let status = resp.status();
            let status_text = status.canonical_reason().unwrap_or("Unknown").to_string();
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            Ok(Response {
                status: status.as_u16(),
                status_text,
                body,
            })
        }
        Err(e) => Err(HttpError::Network(e.to_string())),
    }
}

/// Send request for methods that normally don't have a body (GET, DELETE, HEAD, OPTIONS)
fn send_request(
    builder: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    body: Option<String>,
    content_type: Option<&str>,
    auth: Option<&str>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let builder = add_headers(builder, content_type, auth);
    match body {
        Some(b) => builder.force_send_body().send(b),
        None => builder.call(),
    }
}

/// Send request for methods that have a body (POST, PUT, PATCH)
fn send_request_with_body(
    builder: ureq::RequestBuilder<ureq::typestate::WithBody>,
    body: Option<String>,
    content_type: Option<&str>,
    auth: Option<&str>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let builder = add_headers(builder, content_type, auth);
    match body {
        Some(b) => builder.send(b),
        None => builder.send_empty(),
    }
}

fn add_headers<T>(
    mut builder: ureq::RequestBuilder<T>,
    content_type: Option<&str>,
    auth: Option<&str>,
) -> ureq::RequestBuilder<T> {
    if let Some(ct) = content_type {
        builder = builder.header("Content-Type", ct);
    }
    if let Some(a) = auth {
        builder = builder.header("Authorization", a);
    }
    builder
}

fn build_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    format!("{}{}", base, path)
}

fn is_ndjson_endpoint(path: &str) -> bool {
    // Check for _bulk or _msearch in path (before query string)
    let path_part = path.split('?').next().unwrap_or(path);
    path_part.contains("_bulk") || path_part.contains("_msearch")
}

/// Reads one HTTP request (headers plus Content-Length body) from a test client.
/// A single read() can return before the body arrives, which made tests flaky.
#[cfg(test)]
pub(crate) fn read_test_request(stream: &mut impl std::io::Read) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end]
                .lines()
                .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            if data.len() >= end + 4 + len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
                        let req = read_test_request(&mut stream);
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
    fn test_build_url_with_leading_slash() {
        assert_eq!(
            build_url("http://localhost:9200", "/_search"),
            "http://localhost:9200/_search"
        );
    }

    #[test]
    fn test_build_url_without_leading_slash() {
        assert_eq!(
            build_url("http://localhost:9200", "_search"),
            "http://localhost:9200/_search"
        );
    }

    #[test]
    fn test_build_url_base_with_trailing_slash() {
        assert_eq!(
            build_url("http://localhost:9200/", "/_search"),
            "http://localhost:9200/_search"
        );
    }

    #[test]
    fn test_build_url_preserves_query_string() {
        assert_eq!(
            build_url("http://localhost:9200", "/_cat/indices?v=true"),
            "http://localhost:9200/_cat/indices?v=true"
        );
    }

    #[test]
    fn test_is_ndjson_endpoint() {
        assert!(is_ndjson_endpoint("/_bulk"));
        assert!(is_ndjson_endpoint("/myindex/_bulk"));
        assert!(is_ndjson_endpoint("/_msearch"));
        assert!(is_ndjson_endpoint("/_bulk?refresh=true"));
        assert!(!is_ndjson_endpoint("/_search"));
        assert!(!is_ndjson_endpoint("/_cat/indices"));
    }

    #[test]
    fn test_simple_get() {
        let server = MockServer::start(|_| ok_response(r#"{"version":"8.0.0"}"#));
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let req = Request {
            method: "GET".to_string(),
            path: "/".to_string(),
            bodies: vec![],
        };

        let resp = execute(&conn, &req).unwrap();
        assert_eq!(resp.status, 200);
        assert!(resp.body.contains("version"));
    }

    #[test]
    fn test_api_key_auth_header() {
        let server = MockServer::start(|req| {
            // http crate normalizes header names to lowercase
            if req.to_lowercase().contains("authorization: apikey mykey123") {
                ok_response(r#"{"auth":"ok"}"#)
            } else {
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_string()
            }
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::ApiKey("mykey123".to_string()),
        };
        let req = Request {
            method: "GET".to_string(),
            path: "/".to_string(),
            bodies: vec![],
        };

        let resp = execute(&conn, &req).unwrap();
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn test_basic_auth_header() {
        let server = MockServer::start(|req| {
            // base64("user:pass") = "dXNlcjpwYXNz"; http crate lowercases headers
            if req.to_lowercase().contains("authorization: basic dxnlcjpwyxnz") {
                ok_response(r#"{"auth":"ok"}"#)
            } else {
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_string()
            }
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::Basic {
                username: "user".to_string(),
                password: "pass".to_string(),
            },
        };
        let req = Request {
            method: "GET".to_string(),
            path: "/".to_string(),
            bodies: vec![],
        };

        let resp = execute(&conn, &req).unwrap();
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn test_ndjson_content_type_for_bulk() {
        let server = MockServer::start(|req| {
            // http crate lowercases headers
            if req.to_lowercase().contains("content-type: application/x-ndjson") {
                ok_response(r#"{"took":1}"#)
            } else {
                ok_response(r#"{"error":"wrong content type"}"#)
            }
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let req = Request {
            method: "POST".to_string(),
            path: "/_bulk".to_string(),
            bodies: vec![
                r#"{"index":{}}"#.to_string(),
                r#"{"field":"value"}"#.to_string(),
            ],
        };

        let resp = execute(&conn, &req).unwrap();
        assert!(resp.body.contains("took"));
    }

    #[test]
    fn test_ndjson_body_format() {
        let server = MockServer::start(|req| {
            // Check body has newlines between objects and trailing newline
            if req.contains("{\"a\":1}\n{\"b\":2}\n") {
                ok_response(r#"{"ok":true}"#)
            } else {
                ok_response(&format!(r#"{{"error":"bad body","req":{:?}}}"#, req))
            }
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let req = Request {
            method: "POST".to_string(),
            path: "/_bulk".to_string(),
            bodies: vec![r#"{"a":1}"#.to_string(), r#"{"b":2}"#.to_string()],
        };

        let resp = execute(&conn, &req).unwrap();
        assert!(resp.body.contains("ok"));
    }

    #[test]
    fn test_json_content_type_for_search() {
        let server = MockServer::start(|req| {
            // http crate lowercases headers
            if req.to_lowercase().contains("content-type: application/json") {
                ok_response(r#"{"hits":[]}"#)
            } else {
                ok_response(r#"{"error":"wrong content type"}"#)
            }
        });
        let conn = Connection {
            url: server.url(),
            auth: Auth::None,
        };
        let req = Request {
            method: "POST".to_string(),
            path: "/_search".to_string(),
            bodies: vec![r#"{"query":{}}"#.to_string()],
        };

        let resp = execute(&conn, &req).unwrap();
        assert!(resp.body.contains("hits"));
    }
}
