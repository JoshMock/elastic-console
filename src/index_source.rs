//! Live index name source from cluster via `GET /_resolve/index/*`.
//!
//! Lazy fetch on first access, cached for 60 seconds. Fetch failures cache
//! an empty list to avoid retry storms.

use crate::{Auth, Connection};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Parses `_resolve/index/*` response JSON, extracts sorted unique names.
pub fn parse_resolve_response(body: &str) -> Vec<String> {
    // Response shape: {"indices":[{"name":..}], "aliases":[{"name":..}], "data_streams":[{"name":..}]}
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return vec![];
    };

    let mut names = std::collections::BTreeSet::new();

    for key in ["indices", "aliases", "data_streams"] {
        if let Some(arr) = json.get(key).and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    names.insert(name.to_string());
                }
            }
        }
    }

    names.into_iter().collect()
}

/// Fetches index names from the cluster. Returns empty vec on any error.
fn fetch_index_names(conn: &Connection) -> Vec<String> {
    let url = format!("{}/_resolve/index/*", conn.url.trim_end_matches('/'));

    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(FETCH_TIMEOUT))
        .build();
    let agent: ureq::Agent = config.into();

    let mut builder = agent.get(&url);

    // Add auth header
    match &conn.auth {
        Auth::ApiKey(key) => {
            builder = builder.header("Authorization", format!("ApiKey {}", key));
        }
        Auth::Basic { username, password } => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let encoded = STANDARD.encode(format!("{}:{}", username, password));
            builder = builder.header("Authorization", format!("Basic {}", encoded));
        }
        Auth::None => {}
    }

    match builder.call() {
        Ok(mut resp) if resp.status().is_success() => {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            parse_resolve_response(&body)
        }
        _ => vec![], // ponytail: silent fallback on any error
    }
}

/// Index name source with caching. Inject `fetch_fn` for testing.
pub struct IndexSource<F> {
    fetch_fn: F,
    cache: Option<(Instant, Vec<String>)>,
    clock: fn() -> Instant,
}

impl IndexSource<Box<dyn Fn() -> Vec<String>>> {
    /// Create a real IndexSource that fetches from the cluster.
    pub fn new(conn: Connection) -> Self {
        Self {
            fetch_fn: Box::new(move || fetch_index_names(&conn)),
            cache: None,
            clock: Instant::now,
        }
    }
}

impl<F: Fn() -> Vec<String>> IndexSource<F> {
    /// Create with injected fetch function (for testing).
    #[cfg(test)]
    pub fn with_fetch(fetch_fn: F, clock: fn() -> Instant) -> Self {
        Self {
            fetch_fn,
            cache: None,
            clock,
        }
    }

    /// Get index names, fetching if cache is empty or stale.
    pub fn get_names(&mut self) -> &[String] {
        let now = (self.clock)();
        let needs_fetch = match &self.cache {
            None => true,
            Some((cached_at, _)) => now.duration_since(*cached_at) >= CACHE_TTL,
        };

        if needs_fetch {
            let names = (self.fetch_fn)();
            self.cache = Some((now, names));
        }

        &self.cache.as_ref().unwrap().1
    }

    /// Filter names by prefix.
    pub fn names_with_prefix(&mut self, prefix: &str) -> Vec<String> {
        self.get_names()
            .iter()
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn parse_response_extracts_all_names_sorted() {
        let json = r#"{
            "indices": [{"name": "logs-2024"}, {"name": "metrics"}],
            "aliases": [{"name": "logs"}],
            "data_streams": [{"name": "logs-ds"}]
        }"#;
        let names = parse_resolve_response(json);
        assert_eq!(names, vec!["logs", "logs-2024", "logs-ds", "metrics"]);
    }

    #[test]
    fn parse_response_dedupes() {
        let json = r#"{
            "indices": [{"name": "shared"}],
            "aliases": [{"name": "shared"}]
        }"#;
        let names = parse_resolve_response(json);
        assert_eq!(names, vec!["shared"]);
    }

    #[test]
    fn parse_response_handles_malformed_json() {
        assert_eq!(parse_resolve_response("not json"), Vec::<String>::new());
        assert_eq!(parse_resolve_response("{}"), Vec::<String>::new());
    }

    #[test]
    fn prefix_filtering() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = IndexSource::with_fetch(
            move || {
                cc.fetch_add(1, Ordering::SeqCst);
                vec!["my-logs".into(), "my-metrics".into(), "other".into()]
            },
            Instant::now,
        );

        let filtered = source.names_with_prefix("my-");
        assert_eq!(filtered, vec!["my-logs", "my-metrics"]);
    }

    #[test]
    fn cache_hit_within_ttl_does_not_refetch() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = IndexSource::with_fetch(
            move || {
                cc.fetch_add(1, Ordering::SeqCst);
                vec!["idx".into()]
            },
            Instant::now,
        );

        // First call fetches
        source.get_names();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call uses cache
        source.get_names();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_expiry_triggers_refetch() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        // Use a fake clock that jumps forward
        thread_local! {
            static FAKE_TIME: Cell<Instant> = Cell::new(Instant::now());
        }
        fn fake_now() -> Instant {
            FAKE_TIME.with(|t| t.get())
        }

        let mut source = IndexSource::with_fetch(
            move || {
                cc.fetch_add(1, Ordering::SeqCst);
                vec!["idx".into()]
            },
            fake_now,
        );

        // First fetch
        source.get_names();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Advance time past TTL
        FAKE_TIME.with(|t| t.set(Instant::now() + Duration::from_secs(61)));

        // Should refetch
        source.get_names();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fetch_error_returns_empty_and_caches() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = IndexSource::with_fetch(
            move || {
                cc.fetch_add(1, Ordering::SeqCst);
                vec![] // Simulates error (fetch_index_names returns empty on error)
            },
            Instant::now,
        );

        // First call: "error" (empty)
        let names = source.get_names();
        assert!(names.is_empty());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call within TTL: should use cached empty, not retry
        let names = source.get_names();
        assert!(names.is_empty());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn end_to_end_resolve_index_request() {
        use crate::http::{ok_response, MockServer};
        use std::sync::atomic::AtomicBool;

        let got_correct_request = Arc::new(AtomicBool::new(false));
        let flag = got_correct_request.clone();

        let server = MockServer::start(move |req| {
            // Verify: GET /_resolve/index/* with auth header
            let is_get = req.starts_with("GET ");
            let has_path = req.contains("/_resolve/index/*");
            let has_auth = req.to_lowercase().contains("authorization: apikey testkey");
            if is_get && has_path && has_auth {
                flag.store(true, Ordering::SeqCst);
            }
            ok_response(r#"{"indices":[{"name":"live-idx"}],"aliases":[],"data_streams":[]}"#)
        });

        let conn = Connection {
            url: server.url(),
            auth: Auth::ApiKey("testkey".into()),
        };
        let names = fetch_index_names(&conn);

        assert!(got_correct_request.load(Ordering::SeqCst), "expected GET /_resolve/index/* with auth");
        assert_eq!(names, vec!["live-idx"]);
    }
}
