//! Cookies, as the `Storage` and `Network` domains present them.
//!
//! A page's jar here is `document.cookie`: name and value, nothing else. The
//! attributes a client sends — domain, path, expiry, `httpOnly` — are accepted
//! and dropped, because there is no persistence layer for them to mean anything
//! to. What is reported back therefore describes the page asking, not the
//! server that set the cookie.

use serde_json::{Value, json};
use url::Url;

/// Cookies a connection may hold. One jar per connection is the whole model.
const MAX_COOKIES: usize = 200;

pub fn parse(raw: &str) -> Vec<(String, String)> {
    raw.split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

pub fn serialize(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `Network.Cookie` for each pair, attributed to the page holding them.
pub fn describe(pairs: &[(String, String)], page_url: &str) -> Value {
    let domain = Url::parse(page_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let list: Vec<Value> = pairs
        .iter()
        .map(|(name, value)| {
            json!({
                "name": name,
                "value": value,
                "domain": domain,
                "path": "/",
                // -1 is CDP for "goes away with the session", which is the only
                // lifetime a cookie can have here.
                "expires": -1.0,
                "size": name.len() + value.len(),
                "httpOnly": false,
                "secure": page_url.starts_with("https:"),
                "session": true,
                "sameSite": "Lax",
                "priority": "Medium",
                "sourceScheme": "Secure",
                "sourcePort": 443,
            })
        })
        .collect();
    Value::Array(list)
}

/// Apply one `Network.CookieParam`.
pub fn set(pairs: &mut Vec<(String, String)>, cookie: &Value) {
    let Some(name) = cookie.get("name").and_then(Value::as_str) else {
        return;
    };
    if name.is_empty() {
        return;
    }
    let value = cookie
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match pairs.iter().position(|(existing, _)| existing == name) {
        Some(i) => pairs[i].1 = value,
        None if pairs.len() < MAX_COOKIES => pairs.push((name.to_owned(), value)),
        None => tracing::debug!(name, "cookie jar is full"),
    }
}

/// Apply one `Storage.deleteCookies` entry. Only the name is matched: domain
/// and path are not recorded, so honouring them would mean inventing an answer.
pub fn delete(pairs: &mut Vec<(String, String)>, params: &Value) {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return;
    };
    pairs.retain(|(existing, _)| existing != name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jar_round_trips_through_the_document_cookie_string() {
        let pairs = parse(" a=1; b = two ; ; broken ");
        assert_eq!(
            pairs,
            vec![("a".into(), "1".into()), ("b".into(), "two".into())]
        );
        assert_eq!(serialize(&pairs), "a=1; b=two");
    }

    #[test]
    fn setting_a_cookie_replaces_a_name_it_already_holds() {
        let mut pairs = parse("sid=1");
        set(&mut pairs, &json!({"name": "sid", "value": "2"}));
        set(&mut pairs, &json!({"name": "other", "value": "x"}));
        set(&mut pairs, &json!({"value": "no name"}));
        assert_eq!(serialize(&pairs), "sid=2; other=x");

        delete(&mut pairs, &json!({"name": "sid"}));
        assert_eq!(serialize(&pairs), "other=x");
    }
}
