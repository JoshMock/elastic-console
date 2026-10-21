use crate::parse::Request;
use std::collections::HashMap;

/// Wraps an ES|QL query in a `POST /_query` request, substituting `${var}`s first.
pub fn to_request(query: &str, vars: &HashMap<String, String>) -> Request {
    // Literal replace: vars::substitute is JSON-aware and would unquote "${n}".
    let query = vars.iter().fold(query.trim().to_string(), |q, (k, v)| q.replace(&format!("${{{k}}}"), v));
    Request {
        method: "POST".to_string(),
        path: "/_query?format=txt".to_string(),
        bodies: vec![serde_json::json!({ "query": query }).to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_query_as_json_body() {
        let req = to_request("FROM logs\n| WHERE msg == \"hi\"\n", &HashMap::new());
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/_query?format=txt");
        assert_eq!(req.bodies, vec![r#"{"query":"FROM logs\n| WHERE msg == \"hi\""}"#]);
    }

    #[test]
    fn substitutes_vars() {
        let vars = HashMap::from([("idx".to_string(), "logs".to_string())]);
        let req = to_request("FROM ${idx} | LIMIT 1", &vars);
        assert_eq!(req.bodies, vec![r#"{"query":"FROM logs | LIMIT 1"}"#]);
    }

    #[test]
    fn substitution_is_literal_inside_quotes() {
        let vars = HashMap::from([("n".to_string(), "5".to_string())]);
        let req = to_request(r#"WHERE n == "${n}" AND m == "${m}""#, &vars);
        assert_eq!(req.bodies, vec![r#"{"query":"WHERE n == \"5\" AND m == \"${m}\""}"#]);
    }
}
