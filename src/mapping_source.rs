//! Field name source from index mappings via `GET /<index>/_mapping`.
//!
//! Lazy fetch on first access per index, cached for 60 seconds.
//! Fetch failures cache an empty list to avoid retry storms.

use crate::{Auth, Connection};
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Parses `_mapping` response JSON, flattens into sorted unique field paths.
/// Handles nested `properties` and multi-fields from `fields`.
pub fn parse_mapping_response(body: &str) -> Vec<String> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return vec![];
    };

    let mut fields = BTreeSet::new();

    // Response shape: {"index_name": {"mappings": {"properties": {...}}}, ...}
    let Some(obj) = json.as_object() else {
        return vec![];
    };

    for (_index_name, index_data) in obj {
        if let Some(props) = index_data
            .get("mappings")
            .and_then(|m| m.get("properties"))
        {
            flatten_properties(props, "", &mut fields);
        }
    }

    fields.into_iter().collect()
}

/// Recursively flatten properties into dotted paths.
fn flatten_properties(props: &serde_json::Value, prefix: &str, out: &mut BTreeSet<String>) {
    let Some(obj) = props.as_object() else {
        return;
    };

    for (name, def) in obj {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{}.{}", prefix, name)
        };

        // If it has a type, it's a leaf field
        if def.get("type").is_some() {
            out.insert(path.clone());
        }

        // Check for nested properties
        if let Some(nested_props) = def.get("properties") {
            flatten_properties(nested_props, &path, out);
        }

        // Check for multi-fields
        if let Some(multi_fields) = def.get("fields") {
            flatten_properties(multi_fields, &path, out);
        }
    }
}

/// Fetches field names for an index from the cluster.
fn fetch_field_names(conn: &Connection, index: &str) -> Vec<String> {
    let url = format!("{}{}/_mapping", conn.url.trim_end_matches('/'), 
        if index.starts_with('/') { index.to_string() } else { format!("/{}", index) });

    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(FETCH_TIMEOUT))
        .build();
    let agent: ureq::Agent = config.into();

    let mut builder = agent.get(&url);

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
            parse_mapping_response(&body)
        }
        _ => vec![], // ponytail: silent fallback on any error
    }
}

/// Field name source with per-index caching.
pub struct MappingSource<F> {
    fetch_fn: F,
    cache: HashMap<String, (Instant, Vec<String>)>,
    clock: fn() -> Instant,
}

impl MappingSource<Box<dyn Fn(&str) -> Vec<String> + Send>> {
    /// Create a real MappingSource that fetches from the cluster.
    pub fn new(conn: Connection) -> Self {
        Self {
            fetch_fn: Box::new(move |index: &str| fetch_field_names(&conn, index)),
            cache: HashMap::new(),
            clock: Instant::now,
        }
    }

    /// Create from an already-boxed fetch function (for testing).
    #[cfg(test)]
    #[allow(clippy::type_complexity)] // ponytail: test-only, matches complete.rs pattern
    pub fn new_from_boxed(fetch_fn: Box<dyn Fn(&str) -> Vec<String> + Send>) -> Self {
        Self {
            fetch_fn,
            cache: HashMap::new(),
            clock: Instant::now,
        }
    }
}

impl<F: Fn(&str) -> Vec<String>> MappingSource<F> {
    /// Create with injected fetch function (for testing).
    #[cfg(test)]
    pub fn with_fetch(fetch_fn: F, clock: fn() -> Instant) -> Self {
        Self {
            fetch_fn,
            cache: HashMap::new(),
            clock,
        }
    }

    /// Get field names for an index, fetching if cache is empty or stale.
    pub fn get_fields(&mut self, index: &str) -> &[String] {
        let now = (self.clock)();
        let needs_fetch = match self.cache.get(index) {
            None => true,
            Some((cached_at, _)) => now.duration_since(*cached_at) >= CACHE_TTL,
        };

        if needs_fetch {
            let fields = (self.fetch_fn)(index);
            self.cache.insert(index.to_string(), (now, fields));
        }

        &self.cache.get(index).unwrap().1
    }

    /// Filter field names by prefix.
    pub fn fields_with_prefix(&mut self, index: &str, prefix: &str) -> Vec<String> {
        self.get_fields(index)
            .iter()
            .filter(|f| f.starts_with(prefix))
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
    fn parse_response_flattens_nested_and_multifields() {
        // Realistic mapping with nested properties and multi-fields
        let json = r#"{"books":{"mappings":{"properties":{"title":{"type":"text","fields":{"keyword":{"type":"keyword"}}},"author":{"properties":{"name":{"type":"text"}}}}}}}"#;
        let fields = parse_mapping_response(json);
        assert_eq!(fields, vec!["author.name", "title", "title.keyword"]);
    }

    #[test]
    fn parse_response_merges_multiple_indices() {
        // Two indices in one response (from wildcard or comma-list)
        let json = r#"{
            "books":{"mappings":{"properties":{"title":{"type":"text"}}}},
            "articles":{"mappings":{"properties":{"headline":{"type":"text"},"title":{"type":"text"}}}}
        }"#;
        let fields = parse_mapping_response(json);
        // Should merge and dedupe, sorted
        assert_eq!(fields, vec!["headline", "title"]);
    }

    #[test]
    fn parse_response_handles_malformed_json() {
        assert_eq!(parse_mapping_response("not json"), Vec::<String>::new());
        assert_eq!(parse_mapping_response("{}"), Vec::<String>::new());
    }

    #[test]
    fn prefix_filtering() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = MappingSource::with_fetch(
            move |_idx: &str| {
                cc.fetch_add(1, Ordering::SeqCst);
                vec!["author.name".into(), "title".into(), "title.keyword".into()]
            },
            Instant::now,
        );

        let filtered = source.fields_with_prefix("books", "ti");
        assert_eq!(filtered, vec!["title", "title.keyword"]);
    }

    #[test]
    fn cache_hit_within_ttl_does_not_refetch() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = MappingSource::with_fetch(
            move |_idx: &str| {
                cc.fetch_add(1, Ordering::SeqCst);
                vec!["field".into()]
            },
            Instant::now,
        );

        // First call fetches
        source.get_fields("books");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call uses cache
        source.get_fields("books");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_expiry_triggers_refetch() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();

        thread_local! {
            static FAKE_TIME: Cell<Instant> = Cell::new(Instant::now());
        }
        fn fake_now() -> Instant {
            FAKE_TIME.with(|t| t.get())
        }

        let mut source = MappingSource::with_fetch(
            move |_idx: &str| {
                cc.fetch_add(1, Ordering::SeqCst);
                vec!["field".into()]
            },
            fake_now,
        );

        // First fetch
        source.get_fields("books");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Advance time past TTL
        FAKE_TIME.with(|t| t.set(Instant::now() + Duration::from_secs(61)));

        // Should refetch
        source.get_fields("books");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fetch_error_returns_empty_and_caches() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = MappingSource::with_fetch(
            move |_idx: &str| {
                cc.fetch_add(1, Ordering::SeqCst);
                vec![] // Simulates error
            },
            Instant::now,
        );

        // First call: "error" (empty)
        let fields = source.get_fields("books");
        assert!(fields.is_empty());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call within TTL: should use cached empty, not retry
        let fields = source.get_fields("books");
        assert!(fields.is_empty());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn different_indices_cached_separately() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let mut source = MappingSource::with_fetch(
            move |idx: &str| {
                cc.fetch_add(1, Ordering::SeqCst);
                vec![format!("{}-field", idx)]
            },
            Instant::now,
        );

        source.get_fields("books");
        source.get_fields("articles");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);

        // Re-fetch same indices uses cache
        source.get_fields("books");
        source.get_fields("articles");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn end_to_end_mapping_request() {
        use crate::http::{ok_response, MockServer};
        use std::sync::atomic::AtomicBool;

        let got_correct_request = Arc::new(AtomicBool::new(false));
        let flag = got_correct_request.clone();

        let server = MockServer::start(move |req| {
            // Verify: GET /books/_mapping with auth header
            let is_get = req.starts_with("GET ");
            let has_path = req.contains("/books/_mapping");
            let has_auth = req.to_lowercase().contains("authorization: apikey testkey");
            if is_get && has_path && has_auth {
                flag.store(true, Ordering::SeqCst);
            }
            ok_response(r#"{"books":{"mappings":{"properties":{"title":{"type":"text"}}}}}"#)
        });

        let conn = Connection {
            url: server.url(),
            auth: Auth::ApiKey("testkey".into()),
        };
        let fields = fetch_field_names(&conn, "books");

        assert!(got_correct_request.load(Ordering::SeqCst), "expected GET /books/_mapping with auth");
        assert_eq!(fields, vec!["title"]);
    }
}
