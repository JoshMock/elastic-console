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
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut request = agent.request(&method, &url);

    // Auth header
    match &conn.auth {
        Auth::ApiKey(key) => {
            request = request.header("Authorization", &format!("ApiKey {}", key));
        }
        Auth::Basic { username, password } => {
            let encoded = STANDARD.encode(format!("{}:{}", username, password));
            request = request.header("Authorization", &format!("Basic {}", encoded));
        }
        Auth::None => {}
    }

    // Content-Type and body
    let body = if req.bodies.is_empty() {
        None
    } else if is_ndjson {
        request = request.header("Content-Type", "application/x-ndjson");
        // NDJSON: each body on its own line, trailing newline
        let mut ndjson = req.bodies.join("\n");
        ndjson.push('\n');
        Some(ndjson)
    } else {
        request = request.header("Content-Type", "application/json");
        Some(req.bodies[0].clone())
    };

    let response = if let Some(body) = body {
        request.send_string(&body)
    } else {
        request.send_empty()
    };

    match response {
        Ok(resp) => {
            let status = resp.status();
            let status_text = resp.status_text().to_string();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            Ok(Response {
                status,
                status_text,
                body,
            })
        }
        Err(e) => Err(HttpError::Network(e.to_string())),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
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
            if req.contains("Authorization: ApiKey mykey123") {
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
            // base64("user:pass") = "dXNlcjpwYXNz"
            if req.contains("Authorization: Basic dXNlcjpwYXNz") {
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
            if req.contains("Content-Type: application/x-ndjson") {
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
            if req.contains("Content-Type: application/json") {
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
