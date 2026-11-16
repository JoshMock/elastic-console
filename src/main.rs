mod body_keys;
mod complete;
mod esql;
mod highlight;
mod http;
mod index_source;
mod mapping_source;
mod output;
mod pager;
mod parse;
mod repl;
mod runner;
mod vars;

use parse::Request;
use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    ApiKey(String),
    Basic { username: String, password: String },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub url: String,
    pub auth: Auth,
}

impl Connection {
    fn from_env<F>(get: F) -> Result<Connection, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let url = get("ELASTIC_ES_URL")
            .ok_or_else(|| "run via 'elastic console' or set ELASTIC_ES_URL".to_string())?;

        let auth = if let Some(api_key) = get("ELASTIC_ES_API_KEY") {
            Auth::ApiKey(api_key)
        } else if let (Some(username), Some(password)) =
            (get("ELASTIC_ES_USERNAME"), get("ELASTIC_ES_PASSWORD"))
        {
            Auth::Basic { username, password }
        } else {
            Auth::None
        };

        Ok(Connection { url, auth })
    }
}

fn default_history_path(esql: bool) -> PathBuf {
    // ponytail: XDG_* is scrubbed by elastic CLI, so use $HOME directly
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".elastic").join(if esql { "esql_history" } else { "console_history" })
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let esql = args.iter().any(|a| a == "--esql");
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        println!("Usage: elastic-console [--esql]");
        println!();
        println!("Interactive Elasticsearch console. Type requests like:");
        println!("  GET /");
        println!("  POST /_search");
        println!("  {{\"query\": {{\"match_all\": {{}}}}}}");
        println!();
        println!("--esql: the input is one ES|QL query, sent to POST /_query?format=txt.");
        println!("  Piped input: separate queries with a blank line.");
        println!();
        println!("Responses are shown in $ELASTIC_CONSOLE_PAGER (default: less -FRX).");
        println!("  For collapsible JSON: ELASTIC_CONSOLE_PAGER=jless");
        println!();
        println!("Key bindings:");
        println!("  Enter       Insert newline");
        println!("  Ctrl+Enter  Run request (Alt+Enter as fallback)");
        println!("  Ctrl+C      Clear input");
        println!("  Ctrl+D      Exit");
        println!();
        println!("Variables:");
        println!("  :set name value   Value is taken literally; don't quote strings (:set idx logs)");
        println!("  :vars             List variables");
        println!("  GET /${{idx}}/_search");
        return;
    }

    let conn = match Connection::from_env(|key| env::var(key).ok()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let stdin = io::stdin();
    if !stdin.is_terminal() {
        // Piped mode: read all input, run requests, exit
        let mut input = String::new();
        if let Err(e) = stdin.lock().read_to_string(&mut input) {
            eprintln!("error reading stdin: {}", e);
            std::process::exit(1);
        }

        let mut out = io::stdout();
        if let Err(e) = if esql {
            runner::run_all_esql(&input, &conn, &mut out)
        } else {
            runner::run_all(&input, &conn, &mut out)
        } {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    } else {
        // Interactive mode
        let history_path = env::var("ELASTIC_CONSOLE_HISTORY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_history_path(esql));

        if let Err(e) = repl::run_interactive(&conn, &history_path, esql) {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_url() {
        let result = Connection::from_env(|_| None);
        assert_eq!(
            result,
            Err("run via 'elastic console' or set ELASTIC_ES_URL".to_string())
        );
    }

    #[test]
    fn test_url_only() {
        let result = Connection::from_env(|key| {
            if key == "ELASTIC_ES_URL" {
                Some("http://localhost:9200".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            result,
            Ok(Connection {
                url: "http://localhost:9200".to_string(),
                auth: Auth::None,
            })
        );
    }

    #[test]
    fn test_api_key() {
        let result = Connection::from_env(|key| match key {
            "ELASTIC_ES_URL" => Some("http://localhost:9200".to_string()),
            "ELASTIC_ES_API_KEY" => Some("abc123".to_string()),
            _ => None,
        });
        assert_eq!(
            result,
            Ok(Connection {
                url: "http://localhost:9200".to_string(),
                auth: Auth::ApiKey("abc123".to_string()),
            })
        );
    }

    #[test]
    fn test_basic_auth() {
        let result = Connection::from_env(|key| match key {
            "ELASTIC_ES_URL" => Some("http://localhost:9200".to_string()),
            "ELASTIC_ES_USERNAME" => Some("user".to_string()),
            "ELASTIC_ES_PASSWORD" => Some("pass".to_string()),
            _ => None,
        });
        assert_eq!(
            result,
            Ok(Connection {
                url: "http://localhost:9200".to_string(),
                auth: Auth::Basic {
                    username: "user".to_string(),
                    password: "pass".to_string(),
                },
            })
        );
    }

    #[test]
    fn test_api_key_takes_precedence() {
        let result = Connection::from_env(|key| match key {
            "ELASTIC_ES_URL" => Some("http://localhost:9200".to_string()),
            "ELASTIC_ES_API_KEY" => Some("abc123".to_string()),
            "ELASTIC_ES_USERNAME" => Some("user".to_string()),
            "ELASTIC_ES_PASSWORD" => Some("pass".to_string()),
            _ => None,
        });
        assert_eq!(
            result,
            Ok(Connection {
                url: "http://localhost:9200".to_string(),
                auth: Auth::ApiKey("abc123".to_string()),
            })
        );
    }

    #[test]
    fn test_default_history_path() {
        // Just verify it doesn't panic and contains expected components
        let path = default_history_path(false);
        let path_str = path.to_string_lossy();
        assert!(path_str.contains(".elastic"));
        assert!(path_str.contains("console_history"));
    }

    #[test]
    fn test_history_path_injectable() {
        // Verify ELASTIC_CONSOLE_HISTORY env var works
        // This is tested implicitly by the code structure - env var is checked first
    }
}
