//! Custom providers: any server that speaks the OpenAI-compatible or the
//! Anthropic Messages API, added by the user with a name, a base URL, an
//! optional key, and optional extra headers.
//!
//! A custom provider is registered at run time and from then on is found by
//! [`crate::find`] like a built-in one, so the agent, the model picker, and
//! the CLI use it with no special case. Its key, like every key, lives in
//! the keychain under `provider.<id>`; this module never stores one.
//!
//! [`parse_config`] reads the JSON a provider's docs hand out, in the
//! shapes people paste:
//!
//! - one provider: `{"name": "…", "baseURL": "…", "apiKey": "…", "headers": {…}}`
//!   (also `base_url`, `baseUrl`, `api_base`, `endpoint`, `url`);
//! - a `models.json` style map: `{"providers": {"<id>": {"baseUrl": "…", "api": "openai-completions", …}}}`;
//! - an opencode style map: `{"provider": {"<id>": {"npm": "@ai-sdk/openai-compatible", "options": {"baseURL": "…"}}}}`;
//! - a list of any of the above.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::catalog::{CATALOG, ProviderSpec, Wire};

/// Which API a custom provider speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomWire {
    #[default]
    OpenAi,
    Anthropic,
}

impl CustomWire {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI-compatible",
            Self::Anthropic => "Anthropic-compatible",
        }
    }

    fn wire(self) -> Wire {
        match self {
            Self::OpenAi => Wire::OpenAiCompatible,
            Self::Anthropic => Wire::Anthropic,
        }
    }
}

/// One custom provider as saved in the preferences (never with its key).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomProvider {
    /// Lowercase letters, digits, and `-`; the keychain name is
    /// `provider.<id>`.
    pub id: String,
    pub name: String,
    pub wire: CustomWire,
    pub base_url: String,
    /// Extra headers sent with every request (an org id, a route).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Whether the server wants a key. A local vLLM or llama.cpp server
    /// usually does not.
    pub needs_key: bool,
    /// Model ids the config listed, shown before the server is asked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// A provider read from pasted JSON, with the key it carried (if any),
/// which the caller saves to the keychain and never to the preferences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub provider: CustomProvider,
    pub key: Option<String>,
}

/// Turns a display name into an id: lowercase ASCII letters and digits,
/// other runs become one `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(40);
    out
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "[::1]"
        || host == "::1"
        || host.starts_with("127.")
}

fn is_private_host(host: &str) -> bool {
    host.starts_with("10.")
        || host.starts_with("192.168.")
        || host
            .strip_prefix("172.")
            .and_then(|r| r.split('.').next())
            .and_then(|o| o.parse::<u8>().ok())
            .is_some_and(|o| (16..=31).contains(&o))
        || host.ends_with(".local")
        || host.ends_with(".lan")
}

/// Checks a custom provider and lists every problem, in words the user can
/// act on. `taken` holds ids already in use by other custom providers.
pub fn validate(p: &CustomProvider, taken: &[&str]) -> Vec<String> {
    let mut errs = Vec::new();
    if p.name.trim().is_empty() {
        errs.push("Give the provider a name.".to_owned());
    }
    if !valid_id(&p.id) {
        errs.push(format!(
            "“{}” cannot be an id: use lowercase letters, digits, and dashes.",
            p.id
        ));
    } else if CATALOG.iter().any(|s| s.id == p.id) {
        errs.push(format!(
            "“{}” is a built-in provider; pick another name.",
            p.id
        ));
    } else if taken.contains(&p.id.as_str()) {
        errs.push(format!(
            "A custom provider called “{}” already exists.",
            p.id
        ));
    }
    let raw = p.base_url.trim();
    match url::Url::parse(raw) {
        Err(_) if raw.is_empty() => errs.push("The base URL is empty.".to_owned()),
        Err(_) => errs.push(format!(
            "“{raw}” is not a URL (it should look like https://api.example.com/v1)."
        )),
        Ok(u) => {
            let host = u.host_str().unwrap_or_default().to_owned();
            match u.scheme() {
                "https" => {}
                "http" if is_loopback_host(&host) || is_private_host(&host) => {}
                "http" => errs.push(
                    "Use https for a server on the internet (plain http is only for this computer or your network)."
                        .to_owned(),
                ),
                other => errs.push(format!(
                    "A base URL starts with https://, not {other}://."
                )),
            }
            if host.is_empty() {
                errs.push("The base URL has no host.".to_owned());
            }
            if u.query().is_some_and(|q| !q.is_empty()) {
                errs.push(
                    "Leave query strings out of the base URL; put keys in the key field."
                        .to_owned(),
                );
            }
        }
    }
    for (k, v) in &p.headers {
        let lower = k.to_ascii_lowercase();
        if k.trim().is_empty()
            || !k
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            errs.push(format!("“{k}” is not a header name."));
        } else if matches!(lower.as_str(), "authorization" | "x-api-key" | "api-key") {
            errs.push(format!(
                "Put the key in the key field instead of the “{k}” header, so it stays in the keychain."
            ));
        }
        if v.contains('\n') || v.contains('\r') {
            errs.push(format!("The “{k}” header's value spans lines."));
        }
    }
    errs
}

/// Reads pasted JSON into providers. Every provider found is checked with
/// [`validate`]; the error lists every problem (with the provider's name in
/// front when there are several), so nothing half-valid is saved.
pub fn parse_config(text: &str, taken: &[&str]) -> Result<Vec<Parsed>, Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| {
        vec![format!(
            "That is not valid JSON (line {}, column {}): {e}",
            e.line(),
            e.column()
        )]
    })?;
    let mut found = Vec::new();
    collect(&v, None, &mut found).map_err(|e| vec![e])?;
    if found.is_empty() {
        return Err(vec![
            "No provider found: expected a base URL (`baseURL`, `base_url`, or `api_base`)."
                .to_owned(),
        ]);
    }
    let mut errs = Vec::new();
    let mut ids: Vec<String> = taken.iter().map(|s| (*s).to_owned()).collect();
    let several = found.len() > 1;
    for f in &found {
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        for e in validate(&f.provider, &refs) {
            errs.push(if several {
                format!("{}: {e}", f.provider.name)
            } else {
                e
            });
        }
        ids.push(f.provider.id.clone());
    }
    if errs.is_empty() {
        Ok(found)
    } else {
        Err(errs)
    }
}

fn str_at<'a>(
    o: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| o.get(*k).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn collect(
    v: &serde_json::Value,
    key_hint: Option<&str>,
    out: &mut Vec<Parsed>,
) -> Result<(), String> {
    if let Some(list) = v.as_array() {
        for item in list {
            collect(item, None, out)?;
        }
        return Ok(());
    }
    let Some(o) = v.as_object() else {
        return Err("Expected a JSON object (or a list of them).".to_owned());
    };
    // A map of providers by id.
    for maps in ["providers", "provider"] {
        if let Some(m) = o.get(maps).and_then(|m| m.as_object()) {
            for (id, p) in m {
                collect(p, Some(id), out)?;
            }
            return Ok(());
        }
    }
    let options = o.get("options").and_then(|x| x.as_object());
    let url_keys = [
        "baseURL", "baseUrl", "base_url", "api_base", "apiBase", "endpoint", "url",
    ];
    let Some(base) = str_at(o, &url_keys).or_else(|| options.and_then(|op| str_at(op, &url_keys)))
    else {
        return Ok(());
    };
    let name = str_at(o, &["name", "displayName", "title"])
        .or(key_hint)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            url::Url::parse(base)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| "Custom".to_owned())
        });
    let id = key_hint
        .map(slug)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| slug(&name));
    let api = str_at(o, &["api", "type", "wire", "format", "npm", "sdk"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let wire = if api.contains("anthropic") {
        CustomWire::Anthropic
    } else {
        CustomWire::OpenAi
    };
    let key = str_at(o, &["apiKey", "api_key", "key", "token"])
        .or_else(|| options.and_then(|op| str_at(op, &["apiKey", "api_key"])))
        // `${ENV}` and `{env:VAR}` placeholders are not keys.
        .filter(|k| !k.starts_with('$') && !k.starts_with("{env"))
        .map(str::to_owned);
    let mut headers = BTreeMap::new();
    for src in [o.get("headers"), options.and_then(|op| op.get("headers"))]
        .into_iter()
        .flatten()
    {
        if let Some(h) = src.as_object() {
            for (k, v) in h {
                if let Some(s) = v.as_str() {
                    headers.insert(k.clone(), s.to_owned());
                }
            }
        }
    }
    let models = match o.get("models") {
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|m| {
                m.as_str().map(str::to_owned).or_else(|| {
                    m.get("id")
                        .or_else(|| m.get("name"))
                        .and_then(|x| x.as_str())
                        .map(str::to_owned)
                })
            })
            .collect(),
        Some(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let remote = url::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(|h| !is_loopback_host(h)))
        .unwrap_or(true);
    let needs_key = key.is_some()
        || o.get("needsKey")
            .and_then(|x| x.as_bool())
            .unwrap_or(remote);
    out.push(Parsed {
        provider: CustomProvider {
            id,
            name,
            wire,
            base_url: base.trim_end_matches('/').to_owned(),
            headers,
            needs_key,
            models,
        },
        key,
    });
    Ok(())
}

// ----- the run-time registry -----

struct Registered {
    spec: &'static ProviderSpec,
    headers: Vec<(String, String)>,
}

fn registry() -> &'static Mutex<Vec<Registered>> {
    static R: OnceLock<Mutex<Vec<Registered>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// Makes `providers` findable by [`crate::find`]. Calling it again with the
/// same list is cheap (a provider whose details did not change keeps its
/// spec); a provider no longer listed stops being found. Built-in ids are
/// never shadowed.
pub fn register_all(providers: &[CustomProvider]) {
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    let mut next = Vec::with_capacity(providers.len());
    for p in providers {
        if !valid_id(&p.id) || CATALOG.iter().any(|s| s.id == p.id) {
            continue;
        }
        let headers: Vec<(String, String)> = p
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let base = p.base_url.trim().trim_end_matches('/');
        let local = url::Url::parse(base)
            .ok()
            .and_then(|u| u.host_str().map(is_loopback_host))
            .unwrap_or(false);
        let same = reg.iter().find(|r| {
            r.spec.id == p.id
                && r.spec.name == p.name
                && r.spec.base_url == base
                && r.spec.wire == p.wire.wire()
                && r.spec.needs_key == p.needs_key
        });
        let spec = match same {
            Some(r) => r.spec,
            None => Box::leak(Box::new(ProviderSpec {
                id: leak(&p.id),
                name: leak(&p.name),
                wire: p.wire.wire(),
                base_url: leak(base),
                needs_key: p.needs_key,
                local: local && !p.needs_key,
                key_url: "",
            })),
        };
        next.push(Registered { spec, headers });
    }
    *reg = next;
}

/// A registered custom provider by id, case-insensitively.
pub fn find(id: &str) -> Option<&'static ProviderSpec> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|r| r.spec.id.eq_ignore_ascii_case(id))
        .map(|r| r.spec)
}

/// Every registered custom provider, in the order given.
pub fn all() -> Vec<&'static ProviderSpec> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|r| r.spec)
        .collect()
}

/// Whether `id` is a custom provider.
pub fn is_custom(id: &str) -> bool {
    find(id).is_some()
}

/// The extra headers for a custom provider (empty for built-ins).
pub fn headers_for(id: &str) -> Vec<(String, String)> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|r| r.spec.id == id)
        .map(|r| r.headers.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(id: &str, url: &str) -> CustomProvider {
        CustomProvider {
            id: id.into(),
            name: id.into(),
            base_url: url.into(),
            needs_key: true,
            ..Default::default()
        }
    }

    #[test]
    fn slugs_are_keychain_safe() {
        assert_eq!(slug("My vLLM Box!"), "my-vllm-box");
        assert_eq!(slug("  Nebius  AI Studio "), "nebius-ai-studio");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn validation_names_every_problem() {
        assert!(validate(&cp("nebius", "https://api.studio.nebius.ai/v1"), &[]).is_empty());
        assert!(validate(&cp("box", "http://localhost:8000/v1"), &[]).is_empty());
        assert!(validate(&cp("lan", "http://192.168.1.20:8000/v1"), &[]).is_empty());
        let e = validate(&cp("openai", "http://example.com/v1?key=1"), &["x"]);
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(e[0].contains("built-in"));
        assert!(e.iter().any(|m| m.contains("https")));
        assert!(e.iter().any(|m| m.contains("query")));
        assert!(validate(&cp("dup", "https://a.b/v1"), &["dup"])[0].contains("already"));
        assert!(validate(&cp("Bad Id", "https://a.b"), &[])[0].contains("cannot be an id"));
        assert!(validate(&cp("x", "not a url"), &[])[0].contains("not a URL"));
        assert!(validate(&cp("x", "ftp://a.b"), &[])[0].contains("ftp"));
        let mut h = cp("x", "https://a.b/v1");
        h.headers.insert("Authorization".into(), "Bearer k".into());
        h.headers.insert("bad header".into(), "v".into());
        let e = validate(&h, &[]);
        assert!(e.iter().any(|m| m.contains("keychain")));
        assert!(e.iter().any(|m| m.contains("not a header name")));
        let mut n = cp("x", "https://a.b");
        n.name = " ".into();
        assert!(validate(&n, &[])[0].contains("name"));
    }

    #[test]
    fn pasted_configs_in_every_common_shape() {
        // One provider, OpenAI-compatible, with a key and a header.
        let one = r#"{"name": "Nebius", "baseURL": "https://api.studio.nebius.ai/v1/",
            "apiKey": "sk-123", "headers": {"X-Org": "me"}, "models": ["meta/llama-3.3"]}"#;
        let got = parse_config(one, &[]).unwrap();
        assert_eq!(got.len(), 1);
        let p = &got[0].provider;
        assert_eq!(p.id, "nebius");
        assert_eq!(p.base_url, "https://api.studio.nebius.ai/v1");
        assert_eq!(p.wire, CustomWire::OpenAi);
        assert_eq!(p.headers["X-Org"], "me");
        assert_eq!(p.models, vec!["meta/llama-3.3"]);
        assert!(p.needs_key);
        assert_eq!(got[0].key.as_deref(), Some("sk-123"));

        // models.json style map, Anthropic-compatible, env placeholder key.
        let map = r#"{"providers": {"kimi": {"baseUrl": "https://api.moonshot.ai/anthropic",
            "api": "anthropic-messages", "apiKey": "$MOONSHOT_KEY",
            "models": [{"id": "kimi-k2"}]}}}"#;
        let got = parse_config(map, &[]).unwrap();
        assert_eq!(got[0].provider.id, "kimi");
        assert_eq!(got[0].provider.wire, CustomWire::Anthropic);
        assert_eq!(got[0].key, None);
        assert_eq!(got[0].provider.models, vec!["kimi-k2"]);

        // opencode style, local server without a key.
        let oc = r#"{"provider": {"vllm": {"npm": "@ai-sdk/openai-compatible", "name": "vLLM",
            "options": {"baseURL": "http://localhost:8000/v1"}, "models": {"qwen3": {}}}}}"#;
        let got = parse_config(oc, &[]).unwrap();
        assert_eq!(got[0].provider.name, "vLLM");
        assert!(!got[0].provider.needs_key);
        assert_eq!(got[0].provider.models, vec!["qwen3"]);

        // A list.
        let list = r#"[{"name": "A", "base_url": "https://a.example/v1"},
                       {"name": "B", "api_base": "https://b.example/v1"}]"#;
        assert_eq!(parse_config(list, &[]).unwrap().len(), 2);
    }

    #[test]
    fn bad_pastes_say_why() {
        let e = parse_config("{nope", &[]).unwrap_err();
        assert!(e[0].contains("not valid JSON") && e[0].contains("line 1"));
        let e = parse_config(r#"{"name": "x"}"#, &[]).unwrap_err();
        assert!(e[0].contains("No provider found"));
        let e = parse_config(
            r#"[{"name": "A", "url": "http://example.com"}, {"name": "B", "url": "x"}]"#,
            &[],
        )
        .unwrap_err();
        assert!(e.iter().any(|m| m.starts_with("A: ")), "{e:?}");
        assert!(e.iter().any(|m| m.starts_with("B: ")), "{e:?}");
        let e = parse_config(r#"{"name": "Taken", "url": "https://t.example"}"#, &["taken"])
            .unwrap_err();
        assert!(e[0].contains("already"));
        assert!(parse_config("42", &[]).is_err());
    }

    #[test]
    fn registered_providers_are_found_like_built_ins() {
        let mut p = cp("zz-test-box", "http://localhost:9999/v1/");
        p.name = "Test box".into();
        p.needs_key = false;
        p.headers.insert("X-Route".into(), "a".into());
        register_all(std::slice::from_ref(&p));
        let spec = crate::find("ZZ-test-box").unwrap();
        assert_eq!(spec.name, "Test box");
        assert_eq!(spec.base_url, "http://localhost:9999/v1");
        assert!(spec.local && !spec.needs_key);
        assert_eq!(
            headers_for("zz-test-box"),
            vec![("X-Route".to_owned(), "a".to_owned())]
        );
        // Same details keep the same spec.
        register_all(std::slice::from_ref(&p));
        assert!(std::ptr::eq(spec, crate::find("zz-test-box").unwrap()));
        assert_eq!(all().len(), 1);
        // A built-in id is never shadowed, and a dropped one is gone.
        register_all(&[cp("openai", "https://evil.example/v1")]);
        assert_eq!(
            crate::find("openai").unwrap().base_url,
            "https://api.openai.com/v1"
        );
        assert!(!is_custom("zz-test-box"));
        register_all(&[]);
    }
}
