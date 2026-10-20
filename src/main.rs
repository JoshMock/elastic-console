use std::env;

#[derive(Debug, PartialEq)]
enum Auth {
    ApiKey(String),
    Basic { username: String, password: String },
    None,
}

#[derive(Debug, PartialEq)]
struct Connection {
    url: String,
    auth: Auth,
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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        println!("Usage: elastic-console");
        return;
    }

    match Connection::from_env(|key| env::var(key).ok()) {
        Ok(conn) => {
            println!("URL: {}", conn.url);
            match conn.auth {
                Auth::ApiKey(_) => println!("Auth: API Key"),
                Auth::Basic { .. } => println!("Auth: Basic"),
                Auth::None => println!("Auth: None"),
            }
        }
        Err(e) => {
            eprintln!("{}", e);
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
            "ELASTIC_ES_USERNAME" => Some("elastic".to_string()),
            "ELASTIC_ES_PASSWORD" => Some("changeme".to_string()),
            _ => None,
        });
        assert_eq!(
            result,
            Ok(Connection {
                url: "http://localhost:9200".to_string(),
                auth: Auth::Basic {
                    username: "elastic".to_string(),
                    password: "changeme".to_string(),
                },
            })
        );
    }

    #[test]
    fn test_api_key_precedence() {
        let result = Connection::from_env(|key| match key {
            "ELASTIC_ES_URL" => Some("http://localhost:9200".to_string()),
            "ELASTIC_ES_API_KEY" => Some("abc123".to_string()),
            "ELASTIC_ES_USERNAME" => Some("elastic".to_string()),
            "ELASTIC_ES_PASSWORD" => Some("changeme".to_string()),
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
}
